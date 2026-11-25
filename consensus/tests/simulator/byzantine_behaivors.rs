use crate::simulator::byzantine::ByzantineProxy;
use hotstuff_consensus::mocks::scheme::TestScheme;
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::network::message::{Message, Recipient};
use hotstuff_consensus::protocol::collector::Collector;
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
use hotstuff_consensus::storage::{KVError, ReadWriteStorage};
use hotstuff_consensus::timer::service::TimerCommand;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::block::Command;
use hotstuff_consensus::types::certificate::BlockQC;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_consensus::types::crypto::Notary;
use hotstuff_consensus::types::vote::Vote;
use hotstuff_types::node::NodeID;
use std::sync::Arc;

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
