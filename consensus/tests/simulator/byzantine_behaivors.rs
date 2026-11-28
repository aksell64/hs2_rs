use crate::simulator::byzantine::ByzantineProxy;
use crate::simulator::simulator::no_effects;
use anyhow::{Context as _, Result, ensure};
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::scheme::{TestCommand, TestDigest, TestScheme};
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::network::message::{Envelope, Message, Recipient};
use hotstuff_consensus::protocol::collector::{Collector, CollectorInput, CollectorOutput};
use hotstuff_consensus::protocol::consensus::{
    Consensus, ConsensusInput, ConsensusOutput, ConsensusResult,
};
use hotstuff_consensus::protocol::driver::Driver;
use hotstuff_consensus::protocol::effects::{Effect, send_to};
use hotstuff_consensus::protocol::leaders::{LeaderPolicy, RoundRobin};
use hotstuff_consensus::protocol::pacemaker::{
    Pacemaker, PacemakerInput, PacemakerOutput, PacemakerResult,
};
use hotstuff_consensus::protocol::timeouts::TimeoutPolicy;
use hotstuff_consensus::protocol::traits;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::storage::staged::StagedStorage;
use hotstuff_consensus::storage::{KVError, ReadStorage, ReadWriteStorage, Storage};
use hotstuff_consensus::timer::service::TimerCommand;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::block::{Block, Command};
use hotstuff_consensus::types::certificate::BlockQC;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_consensus::types::crypto::Notary;
use hotstuff_consensus::types::vote::Vote;
use hotstuff_types::node::NodeID;
use std::collections::HashMap;
use std::sync::Arc;

pub fn silent_proxy<CS: ClockSource>() -> ByzantineProxy<'static, CS> {
    ByzantineProxy::new(
        "silent",
        |_| true,
        |ctx, input| {
            let mut effects = ctx.apply_as_honest(input)?;
            effects.net.clear();
            Ok(effects)
        },
    )
}

pub fn nonconsecutive_commit_proxy<CS: ClockSource + Clone>(
    source: CS,
    policy: FixedPolicy<CS::Clock>,
) -> ByzantineProxy<'static, CS> {
    let mut driver = None;

    ByzantineProxy::new(
        "nonconsecutive_commit",
        |_| true,
        move |ctx, input| {
            let driver = driver.get_or_insert_with(|| {
                let id = ctx.id();
                let n = ctx.state().committee_size();
                let notary = Notary::new(Arc::new(TestScheme { n, id: Some(id) }));
                Driver::with_components(
                    notary.clone(),
                    NonconsecutiveCommitConsensus::new(notary.clone()),
                    Pacemaker::new(
                        notary.clone(),
                        id,
                        source.clone(),
                        FixedPolicy {
                            view_timeout: policy.view_timeout,
                            lock_timeout: policy.lock_timeout,
                        },
                        RoundRobin::new(n),
                    ),
                    Collector::new(notary),
                )
            });
            ctx.force_apply(input, driver)
        },
    )
}

/// Commit with two QCs even when their views are not consecutive.
pub struct NonconsecutiveCommitConsensus {
    inner: Consensus<TestScheme>,
}

impl NonconsecutiveCommitConsensus {
    pub fn new(notary: Notary<TestScheme>) -> Self {
        Self {
            inner: Consensus::new(notary),
        }
    }
}

impl traits::Consensus<TestScheme> for NonconsecutiveCommitConsensus {
    fn apply<C: Command, DB: ReadWriteStorage<TestScheme, C>>(
        &mut self,
        storage: &mut DB,
        cur_view: View,
        high_qc: &BlockQC<TestScheme>,
        event: ConsensusInput<TestScheme, C>,
    ) -> ConsensusResult<TestScheme, C> {
        let ConsensusInput::Update(block) = event else {
            return self.inner.apply(storage, cur_view, high_qc, event);
        };
        if block.justify.view() > storage.get_locked_qc()?.view() {
            storage.update_locked_qc(block.justify.clone())?;
        }
        let Some(parent) = storage.get_block_by_hash(&block.justify.block())? else {
            return Ok(None);
        };
        let Some(target) = storage.get_block_by_hash(&parent.justify.block())? else {
            return Ok(None);
        };
        let exec_height = storage.get_exec_height()?;
        if parent.parent != target.hash || target.height <= exec_height {
            return Ok(None);
        }
        let exec_hash = storage.get_exec_hash()?;
        let depth = (target.height - exec_height) as usize;
        let (hash, height) = (target.hash, target.height);
        let mut branch = storage.get_block_branch(target, depth)?;
        if branch.len() != depth || branch[depth - 1].parent != exec_hash {
            return Ok(None);
        }
        storage.update_exec_hash(hash)?;
        storage.update_exec_height(height)?;
        branch.reverse();
        Ok(Some(ConsensusOutput::Commit(branch)))
    }
}

pub fn unlimited_votes_proxy<CS: ClockSource + Clone>(
    source: CS,
    policy: FixedPolicy<CS::Clock>,
) -> ByzantineProxy<'static, CS> {
    let mut driver = None;

    ByzantineProxy::new(
        "unlimited_votes",
        |_| true,
        move |ctx, input| {
            let driver = driver.get_or_insert_with(|| {
                let id = ctx.id();
                let n = ctx.state().committee_size();
                let notary = Notary::new(Arc::new(TestScheme { n, id: Some(id) }));
                Driver::with_components(
                    notary.clone(),
                    UnlimitedVotesConsensus::new(notary.clone()),
                    Pacemaker::new(
                        notary.clone(),
                        id,
                        source.clone(),
                        FixedPolicy {
                            view_timeout: policy.view_timeout,
                            lock_timeout: policy.lock_timeout,
                        },
                        RoundRobin::new(n),
                    ),
                    Collector::new(notary),
                )
            });
            ctx.force_apply(input, driver)
        },
    )
}

pub struct UnlimitedVotesConsensus {
    inner: Consensus<TestScheme>,
}

impl UnlimitedVotesConsensus {
    pub fn new(notary: Notary<TestScheme>) -> Self {
        Self {
            inner: Consensus::new(notary),
        }
    }
}

impl traits::Consensus<TestScheme> for UnlimitedVotesConsensus {
    fn apply<C: Command, DB: ReadWriteStorage<TestScheme, C>>(
        &mut self,
        storage: &mut DB,
        cur_view: View,
        high_qc: &BlockQC<TestScheme>,
        event: ConsensusInput<TestScheme, C>,
    ) -> ConsensusResult<TestScheme, C> {
        let ConsensusInput::ReceivedProposal(proposal) = event else {
            return self.inner.apply(storage, cur_view, high_qc, event);
        };
        let block = proposal.block;
        if proposal.view != cur_view || block.parent != block.justify.block() {
            return Ok(None);
        }
        let Some(parent) = storage.get_block_by_hash(&block.parent)? else {
            return Ok(None);
        };
        if parent.height.checked_add(1) != Some(block.height) {
            return Ok(None);
        }
        if block.justify.view() < storage.get_locked_qc()?.view() {
            return Ok(Some(ConsensusOutput::RejectVote(block)));
        }
        storage.update_voted_view(cur_view)?;
        Ok(Some(ConsensusOutput::ConfirmVote {
            vote: Vote {
                view: cur_view,
                block: block.hash,
            },
            block,
        }))
    }
}

pub fn unlimited_votes_and_qcs_proxy<CS: ClockSource + Clone>(
    source: CS,
    policy: FixedPolicy<CS::Clock>,
) -> ByzantineProxy<'static, CS> {
    let mut driver = None;

    ByzantineProxy::new(
        "unlimited_votes_and_qcs",
        |_| true,
        move |ctx, input| {
            let driver = driver.get_or_insert_with(|| {
                let id = ctx.id();
                let n = ctx.state().committee_size();
                let notary = Notary::new(Arc::new(TestScheme { n, id: Some(id) }));
                Driver::with_components(
                    notary.clone(),
                    UnlimitedVotesConsensus::new(notary.clone()),
                    Pacemaker::new(
                        notary.clone(),
                        id,
                        source.clone(),
                        FixedPolicy {
                            view_timeout: policy.view_timeout,
                            lock_timeout: policy.lock_timeout,
                        },
                        RoundRobin::new(n),
                    ),
                    UnlimitedQCsCollector::new(notary),
                )
            });
            ctx.force_apply(input, driver)
        },
    )
}

pub struct UnlimitedQCsCollector {
    inner: Collector<TestScheme>,
    stores: HashMap<(View, TestDigest), KVStorage<MemoryKV<Vec<u8>, Vec<u8>>>>,
}

impl UnlimitedQCsCollector {
    pub fn new(notary: Notary<TestScheme>) -> Self {
        Self {
            inner: Collector::new(notary),
            stores: HashMap::new(),
        }
    }
}

impl traits::Collector<TestScheme> for UnlimitedQCsCollector {
    fn apply<C: Command, DB: ReadWriteStorage<TestScheme, C>>(
        &mut self,
        storage: &mut DB,
        event: CollectorInput<TestScheme>,
    ) -> Result<CollectorOutput<TestScheme>, KVError> {
        let CollectorInput::GenericVoteInput(vote) = event else {
            return self.inner.apply(storage, event);
        };
        let store = self
            .stores
            .entry((vote.view(), vote.block_hash()))
            .or_insert_with(|| KVStorage::new(MemoryKV::new()));
        let mut staged = StagedStorage::<TestScheme, C, _>::new(store);
        let output = self
            .inner
            .apply(&mut staged, CollectorInput::GenericVoteInput(vote))?;
        let batch = staged.into_batch();
        Storage::<TestScheme, C>::commit_write_batch(store, batch)?;
        Ok(output)
    }
}

pub fn equivocation_proxy<CS: ClockSource>() -> ByzantineProxy<'static, CS> {
    ByzantineProxy::new(
        "equivocation",
        |_| true,
        |ctx, input| {
            let mut effects = ctx.apply_as_honest(input)?;
            let committee_size = ctx.state().committee_size();
            let notary = Notary::new(Arc::new(TestScheme {
                n: committee_size,
                id: None,
            }));

            for envelope in std::mem::take(&mut effects.net) {
                let Message::ProposalMsg(proposal) = &envelope.msg else {
                    effects.net.push(envelope);
                    continue;
                };

                let mut conflicting = proposal.clone();
                loop {
                    conflicting.block.cmd = TestCommand(ctx.randomness.rand());
                    conflicting.block.hash = notary.block_hash(
                        &conflicting.block.parent,
                        &conflicting.block.cmd,
                        &conflicting.block.justify,
                        conflicting.block.height,
                    );
                    if conflicting.block.hash != proposal.block.hash {
                        break;
                    }
                }

                let recipients: Vec<_> = match envelope.to {
                    Recipient::All => (0..committee_size).map(NodeID::from).collect(),
                    Recipient::Node(id) => vec![id],
                };
                for id in recipients {
                    effects.net.push(Envelope {
                        to: Recipient::Node(id),
                        msg: Message::ProposalMsg(proposal.clone()),
                    });
                    effects.net.push(Envelope {
                        to: Recipient::Node(id),
                        msg: Message::ProposalMsg(conflicting.clone()),
                    });
                }
            }

            Ok(effects)
        },
    )
}

pub fn tail_fork_proxy<CS: ClockSource + Clone>(
    source: CS,
    policy: FixedPolicy<CS::Clock>,
) -> ByzantineProxy<'static, CS> {
    let mut driver = None;

    ByzantineProxy::new(
        "tail_fork",
        |_| true,
        move |ctx, input| {
            let driver = driver.get_or_insert_with(|| {
                let id = ctx.id();
                let n = ctx.state().committee_size();
                let notary = Notary::new(Arc::new(TestScheme { n, id: Some(id) }));
                Driver::with_components(
                    notary.clone(),
                    TailForkConsensus::new(notary.clone()),
                    Pacemaker::new(
                        notary.clone(),
                        id,
                        source.clone(),
                        FixedPolicy {
                            view_timeout: policy.view_timeout,
                            lock_timeout: policy.lock_timeout,
                        },
                        RoundRobin::new(n),
                    ),
                    Collector::new(notary),
                )
            });
            ctx.force_apply(input, driver)
        },
    )
}

pub struct TailForkConsensus {
    inner: Consensus<TestScheme>,
}

impl TailForkConsensus {
    pub fn new(notary: Notary<TestScheme>) -> Self {
        Self {
            inner: Consensus::new(notary),
        }
    }
}

impl traits::Consensus<TestScheme> for TailForkConsensus {
    fn apply<C: Command, DB: ReadWriteStorage<TestScheme, C>>(
        &mut self,
        storage: &mut DB,
        cur_view: View,
        high_qc: &BlockQC<TestScheme>,
        event: ConsensusInput<TestScheme, C>,
    ) -> ConsensusResult<TestScheme, C> {
        if matches!(&event, ConsensusInput::MakeProposal(_)) && high_qc.view() > 0 {
            let tip = storage
                .get_block_by_hash(&high_qc.block())?
                .ok_or_else(|| KVError::Missing {
                    key: high_qc.block().as_ref().to_vec(),
                })?;
            return self.inner.apply(storage, cur_view, &tip.justify, event);
        }
        self.inner.apply(storage, cur_view, high_qc, event)
    }
}

/// Reveal the QC for `view - 1` to one recipient when proposing `view`, then remain silent.
pub fn reveal_qc_to_one_proxy<CS: ClockSource>(
    view: View,
    recipient: NodeID,
) -> ByzantineProxy<'static, CS> {
    let mut revealed = false;
    ByzantineProxy::new(
        "reveal_qc_to_one",
        |_| true,
        move |ctx, input| {
            if revealed {
                return Ok(no_effects());
            }
            let effects = ctx.apply_as_honest(input)?;
            let qc = effects.net.iter().find_map(|envelope| match &envelope.msg {
                Message::ProposalMsg(proposal) if proposal.view == view => {
                    Some(proposal.block.justify.clone())
                }
                _ => None,
            });
            let Some(qc) = qc else {
                return Ok(effects);
            };
            ensure!(
                qc.view() + 1 == view,
                "the leader did not collect the preceding QC"
            );
            revealed = true;
            let mut effects = no_effects();
            effects.net.push(Envelope {
                to: Recipient::Node(recipient),
                msg: Message::QCMsg(qc),
            });
            Ok(effects)
        },
    )
}

/// Given G -> B -> C, withhold QC(C) and propose a sibling of B on genesis at `attack_view`.
/// Report the protected and conflicting block hashes, then remain silent.
pub fn conflicting_proposal_proxy<CS: ClockSource>(
    attack_view: View,
    mut record_attack: impl FnMut(TestDigest, TestDigest) + 'static,
) -> ByzantineProxy<'static, CS> {
    let mut sent_attack = false;
    ByzantineProxy::new(
        "conflicting_proposal",
        |_| true,
        move |ctx, input| {
            // Once the fork has been broadcast, withhold all further help, including QC(C).
            if sent_attack {
                return Ok(no_effects());
            }
            let mut effects = ctx.apply_as_honest(input)?;
            let storage: &dyn ReadStorage<TestScheme, TestCommand> = ctx.storage();
            for envelope in &mut effects.net {
                let Message::ProposalMsg(proposal) = &mut envelope.msg else {
                    continue;
                };
                if proposal.view != attack_view {
                    continue;
                }
                ensure!(
                    proposal.block.justify.view() + 1 == attack_view,
                    "the leader did not collect QC(C) for the preceding view"
                );
                let c = storage
                    .get_block_by_hash(&proposal.block.justify.block())?
                    .context("C is not stored at the leader")?;
                ensure!(
                    c.justify.view() + 1 == proposal.block.justify.view(),
                    "C does not carry QC(B) for the preceding view"
                );
                let b = storage
                    .get_block_by_hash(&c.parent)?
                    .context("B is not stored at the leader")?;
                let genesis = storage
                    .get_block_by_hash(&b.parent)?
                    .context("the parent of B is not stored at the leader")?;
                ensure!(
                    genesis.height == 0 && b.height == 1,
                    "B is not a child of genesis"
                );

                let cmd = proposal.block.cmd.clone();
                let justify = b.justify;
                let notary = Notary::new(Arc::new(TestScheme {
                    n: ctx.state().committee_size(),
                    id: None,
                }));
                let hash = notary.block_hash(&genesis.hash, &cmd, &justify, b.height);
                let fork = Block {
                    parent: genesis.hash,
                    cmd,
                    justify,
                    height: b.height,
                    hash,
                };
                ensure!(
                    fork.hash != b.hash,
                    "the attack did not create a conflicting block"
                );
                ensure!(
                    notary.verify_qc(&fork.justify),
                    "the fork has an invalid QC"
                );
                sent_attack = true;
                record_attack(b.hash, fork.hash);
                proposal.block = fork;
            }
            Ok(effects)
        },
    )
}

pub fn non_monotonic_proxy<CS: ClockSource + Clone>(
    source: CS,
    policy: FixedPolicy<CS::Clock>,
) -> ByzantineProxy<'static, CS> {
    let mut driver = None;

    ByzantineProxy::new(
        "non_monotonic_votes",
        |_| true,
        move |ctx, input| {
            let driver = driver.get_or_insert_with(|| {
                let id = ctx.id();
                let n = ctx.state().committee_size();
                let notary = Notary::new(Arc::new(TestScheme { n, id: Some(id) }));
                Driver::with_components(
                    notary.clone(),
                    NonMonotonicConsensus::new(notary.clone()),
                    Pacemaker::new(
                        notary.clone(),
                        id,
                        source.clone(),
                        FixedPolicy {
                            view_timeout: policy.view_timeout,
                            lock_timeout: policy.lock_timeout,
                        },
                        RoundRobin::new(n),
                    ),
                    Collector::new(notary),
                )
            });
            ctx.force_apply(input, driver)
        },
    )
}

pub struct NonMonotonicConsensus {
    inner: Consensus<TestScheme>,
}

impl NonMonotonicConsensus {
    pub fn new(notary: Notary<TestScheme>) -> Self {
        Self {
            inner: Consensus::new(notary),
        }
    }
}

impl traits::Consensus<TestScheme> for NonMonotonicConsensus {
    fn apply<C: Command, DB: ReadWriteStorage<TestScheme, C>>(
        &mut self,
        storage: &mut DB,
        cur_view: View,
        high_qc: &BlockQC<TestScheme>,
        event: ConsensusInput<TestScheme, C>,
    ) -> ConsensusResult<TestScheme, C> {
        let ConsensusInput::ReceivedProposal(proposal) = event else {
            return self.inner.apply(storage, cur_view, high_qc, event);
        };

        let block = proposal.block;
        if block.parent != block.justify.block() {
            return Ok(None);
        }
        let Some(parent) = storage.get_block_by_hash(&block.parent)? else {
            return Ok(None);
        };
        if parent.height.checked_add(1) != Some(block.height) {
            return Ok(None);
        }
        if block.justify.view() < storage.get_locked_qc()?.view() {
            return Ok(Some(ConsensusOutput::RejectVote(block)));
        }

        storage.update_voted_view(proposal.view)?;
        Ok(Some(ConsensusOutput::ConfirmVote {
            vote: Vote {
                view: proposal.view,
                block: block.hash,
            },
            block,
        }))
    }
}

pub fn no_lock_timer_proxy<CS: ClockSource + Clone>(
    source: CS,
    policy: FixedPolicy<CS::Clock>,
) -> ByzantineProxy<'static, CS> {
    let mut driver = None;

    ByzantineProxy::new(
        "no_lock_timer",
        |_| true,
        move |ctx, input| {
            let driver = driver.get_or_insert_with(|| {
                let id = ctx.id();
                let n = ctx.state().committee_size();
                let notary = Notary::new(Arc::new(TestScheme { n, id: Some(id) }));
                Driver::with_components(
                    notary.clone(),
                    Consensus::new(notary.clone()),
                    NoLockTimerPacemaker::new(
                        notary.clone(),
                        id,
                        source.clone(),
                        FixedPolicy {
                            view_timeout: policy.view_timeout,
                            lock_timeout: policy.lock_timeout,
                        },
                    ),
                    Collector::new(notary),
                )
            });
            ctx.force_apply(input, driver)
        },
    )
}

pub struct NoLockTimerPacemaker<CS: ClockSource> {
    inner: Pacemaker<TestScheme, FixedPolicy<CS::Clock>, CS, RoundRobin>,
    source: CS,
    policy: FixedPolicy<CS::Clock>,
    leaders: RoundRobin,
}

impl<CS: ClockSource + Clone> NoLockTimerPacemaker<CS> {
    pub fn new(
        notary: Notary<TestScheme>,
        id: NodeID,
        source: CS,
        policy: FixedPolicy<CS::Clock>,
    ) -> Self {
        let n = notary.committee_size();
        Self {
            inner: Pacemaker::new(
                notary,
                id,
                source.clone(),
                FixedPolicy {
                    view_timeout: policy.view_timeout,
                    lock_timeout: policy.lock_timeout,
                },
                RoundRobin::new(n),
            ),
            source,
            policy,
            leaders: RoundRobin::new(n),
        }
    }

    fn on_timer_expired<C: Command, DB: ReadWriteStorage<TestScheme, C>>(
        &mut self,
        storage: &mut DB,
        view: View,
    ) -> Result<PacemakerResult<TestScheme, C, CS::Clock>, KVError> {
        if view != storage.get_cur_view()? || self.leaders.is_epoch_start(view + 1) {
            return self
                .inner
                .apply(storage, PacemakerInput::ViewTimerExpired(view));
        }

        let next = view + 1;
        let leader = self.inner.leader(next);
        storage.update_cur_view(next)?;
        storage.update_cur_leader(leader)?;
        storage.clear_statuses(view)?;
        let start = self.source.now();
        let mut effects = vec![
            Effect::Timer(TimerCommand::ClearLockTimer(view)),
            Effect::Timer(TimerCommand::ClearViewTimersBelow(next)),
        ];
        let epoch = self.leaders.epoch_of(next);
        if storage.get_scheduled_epoch()? < Some(epoch) {
            storage.update_scheduled_epoch(epoch)?;
            let deadlines = self.policy.epoch_deadlines(
                self.leaders.epoch_start(next),
                next..=self.leaders.epoch_end(next),
                start,
            );
            effects.push(Effect::Timer(TimerCommand::SetEpochTimers(deadlines)));
        }

        let out = if self.inner.is_leader(next) {
            if !self.inner.can_propose(storage, next)? {
                storage.insert_status(leader, next)?;
            }
            PacemakerOutput::ProposeNow(next)
        } else {
            let high_qc = storage.get_high_qc()?;
            if high_qc.view() + 1 != next {
                effects.push(send_to(
                    Recipient::Node(leader),
                    Message::StatusMsg {
                        qc: high_qc,
                        view: next,
                    },
                ));
            }
            PacemakerOutput::ViewAdvanced(next)
        };
        Ok(PacemakerResult { effects, out })
    }
}

impl<CS: ClockSource + Clone> traits::Pacemaker<TestScheme> for NoLockTimerPacemaker<CS> {
    type Clock = CS::Clock;

    fn apply<C: Command, DB: ReadWriteStorage<TestScheme, C>>(
        &mut self,
        storage: &mut DB,
        event: PacemakerInput<TestScheme>,
    ) -> Result<PacemakerResult<TestScheme, C, Self::Clock>, KVError> {
        match event {
            PacemakerInput::ViewTimerExpired(view) => self.on_timer_expired(storage, view),
            event => self.inner.apply(storage, event),
        }
    }

    fn leader(&self, view: View) -> NodeID {
        self.inner.leader(view)
    }

    fn is_leader(&self, view: View) -> bool {
        self.inner.is_leader(view)
    }

    fn is_epoch_leader(&self, view: View) -> bool {
        self.inner.is_epoch_leader(view)
    }
}
