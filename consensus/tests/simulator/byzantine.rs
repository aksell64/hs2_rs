use crate::simulator::rng::Randomness;
use crate::simulator::simulator::{Context, GlobalState, NodeEffects, Replica, no_effects};
use anyhow::{Context as _, Result, ensure};
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::scheme::{TestCommand, TestDigest, TestScheme};
use hotstuff_consensus::network::message::{Envelope, Message, Recipient};
use hotstuff_consensus::protocol::collector::{
    CertificateOutput, Collector, CollectorInput, CollectorOutput,
};
use hotstuff_consensus::protocol::driver::DriverInput;
use hotstuff_consensus::protocol::leaders::{LeaderPolicy, RoundRobin};
use hotstuff_consensus::sm::Effects;
use hotstuff_consensus::storage::{ReadStorage, Storage, WriteStorage};
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::block::{Block, Proposal};
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_consensus::types::crypto::Notary;
use hotstuff_types::node::NodeID;
use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

pub type Input = DriverInput<TestScheme, TestCommand>;
pub type Behavior<'a, CS> =
    Box<dyn FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> + 'a>;

pub struct ByzantineProxy<'a, CS: ClockSource> {
    pub name: &'static str,
    pub activate_when: Box<dyn FnMut(&Context<'_, CS>) -> bool + 'a>,
    pub behavior: Behavior<'a, CS>,
}

impl<'a, CS: ClockSource> ByzantineProxy<'a, CS> {
    pub fn new(
        name: &'static str,
        activate_when: impl FnMut(&Context<'_, CS>) -> bool + 'a,
        behavior: impl FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> + 'a,
    ) -> Self {
        Self {
            name,
            activate_when: Box::new(activate_when),
            behavior: Box::new(behavior),
        }
    }
}

pub fn chained_proxy<'a, CS: ClockSource>(
    byzantines: Vec<&'a mut ByzantineProxy<'_, CS>>,
    mut on_behavior: impl FnMut(&'static str) + 'a,
) -> ByzantineProxy<'a, CS> {
    let mut activations = Vec::new();
    let mut behaviors = Vec::new();
    for node in byzantines {
        activations.push(&mut node.activate_when);
        behaviors.push((node.name, &mut node.behavior));
    }

    // Remember each predicate's result so stateful predicates run once per event.
    let active: Rc<Vec<Cell<bool>>> =
        Rc::new((0..activations.len()).map(|_| Cell::new(false)).collect());
    let activation_flags = Rc::clone(&active);
    let activate_when = move |cx: &Context<'_, CS>| -> bool {
        let mut any_active = false;
        for (activate, flag) in activations.iter_mut().zip(activation_flags.iter()) {
            let enabled = activate(cx);
            flag.set(enabled);
            any_active |= enabled;
        }
        any_active
    };

    let behavior =
        move |cx: &mut ByzantineContext<'_, CS>, input: Input| -> Result<NodeEffects<CS>> {
            let mut effects = Effects::new();
            for ((name, behavior), flag) in behaviors.iter_mut().zip(active.iter()) {
                if !flag.get() {
                    continue;
                }
                on_behavior(name);
                let node_effects = behavior(cx, input.clone())?;
                effects.merge(node_effects);
            }
            Ok(effects)
        };

    ByzantineProxy::new("chained", activate_when, behavior)
}

pub struct ByzantineContext<'a, CS: ClockSource> {
    id: NodeID,
    replica: &'a mut Replica<CS>,
    state: &'a GlobalState<CS>,
    pub randomness: Randomness,
}

impl<'a, CS: ClockSource> ByzantineContext<'a, CS> {
    pub fn new(
        id: NodeID,
        replica: &'a mut Replica<CS>,
        state: &'a GlobalState<CS>,
        randomness: Randomness,
    ) -> Self {
        Self {
            id,
            replica,
            state,
            randomness,
        }
    }

    pub fn id(&self) -> NodeID {
        self.id
    }

    pub fn state(&self) -> &'a GlobalState<CS> {
        self.state
    }

    pub fn storage(&self) -> &KVStorage<MemoryKV<Vec<u8>, Vec<u8>>> {
        self.replica.storage()
    }

    pub fn apply_as_honest(&mut self, input: Input) -> Result<NodeEffects<CS>> {
        self.replica.push_front_input(input);
        Ok(self.replica.apply_next()?.unwrap_or_else(no_effects))
    }

    pub fn inject(&mut self, input: Input) {
        self.replica.add_event_to_queue(input);
    }
}

/// Commit with two QCs even when their views are not consecutive.
pub fn nonconsecutive_commit<CS: ClockSource>()
-> impl FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> {
    |ctx, input| {
        let (block, proposal_view) = match &input {
            DriverInput::NewProposal { sender, proposal }
                if *sender == RoundRobin::new(ctx.state().committee_size()).leader(proposal.view)
                    && proposal.block.justify.view() <= proposal.view =>
            {
                (&proposal.block, Some(proposal.view))
            }
            DriverInput::BlockSynced(block) => (block, None),
            _ => return ctx.apply_as_honest(input),
        };
        let notary = Notary::new(Arc::new(TestScheme {
            n: ctx.state().committee_size(),
            id: None,
        }));
        let valid = notary.verify_block_hash(block)
            && notary.verify_qc(&block.justify)
            && block.parent == block.justify.block();
        let hash = block.hash;
        let mut effects = ctx.apply_as_honest(input)?;
        if !valid {
            return Ok(effects);
        }

        let storage: &dyn ReadStorage<TestScheme, TestCommand> = ctx.storage();
        if let Some(view) = proposal_view {
            if view != storage.get_cur_view()? {
                return Ok(effects);
            }
        }
        // A buffered or rejected input must not commit merely because it arrived.
        let Some(block) = storage.get_block_by_hash(&hash)? else {
            return Ok(effects);
        };
        let Some(parent) = storage.get_block_by_hash(&block.justify.block())? else {
            return Ok(effects);
        };
        if block.height != parent.height + 1 {
            return Ok(effects);
        }
        let Some(target) = storage.get_block_by_hash(&parent.justify.block())? else {
            return Ok(effects);
        };
        let exec_height = storage.get_exec_height()?;
        if parent.parent != target.hash || target.height <= exec_height {
            return Ok(effects);
        }

        // Keep commit_up_to's ancestry checks; only QC view adjacency is omitted.
        let exec_hash = storage.get_exec_hash()?;
        let depth = (target.height - exec_height) as usize;
        let (hash, height) = (target.hash, target.height);
        let mut branch = storage.get_block_branch(target, depth)?;
        if branch.len() != depth || branch[depth - 1].parent != exec_hash {
            return Ok(effects);
        }
        branch.reverse();

        let storage = ctx.replica.storage_mut();
        let mut batch = Storage::<TestScheme, TestCommand>::new_write_batch(storage);
        WriteStorage::<TestScheme, TestCommand>::update_exec_hash(&mut batch, hash)?;
        WriteStorage::<TestScheme, TestCommand>::update_exec_height(&mut batch, height)?;
        for block in &branch {
            batch.commit_block(block)?;
        }
        Storage::<TestScheme, TestCommand>::commit_write_batch(storage, batch)?;
        effects.committed.extend(branch);
        Ok(effects)
    }
}

pub fn unlimited_votes<CS: ClockSource>()
-> impl FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> {
    |ctx, input| {
        if matches!(&input, DriverInput::NewProposal { .. }) {
            // Forget previous votes, including before replayed proposals. All other
            // proposal checks, including the lock, still run in the normal driver.
            let storage = ctx.replica.storage_mut();
            let mut batch = Storage::<TestScheme, TestCommand>::new_write_batch(storage);
            WriteStorage::<TestScheme, TestCommand>::update_voted_view(&mut batch, 0)?;
            Storage::<TestScheme, TestCommand>::commit_write_batch(storage, batch)?;
        }
        ctx.apply_as_honest(input)
    }
}

pub fn unlimited_votes_and_qcs<CS: ClockSource>()
-> impl FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> {
    let mut vote_as_mutant = unlimited_votes();
    let mut collectors: HashMap<(View, TestDigest), Collector<TestScheme>> = HashMap::new();
    move |ctx, input| {
        let DriverInput::NewVote(vote) = input else {
            return vote_as_mutant(ctx, input);
        };
        let n = ctx.state().committee_size();
        let notary = Notary::new(Arc::new(TestScheme { n, id: None }));
        if RoundRobin::new(n).leader(vote.view() + 1) != ctx.id()
            || !notary.verify_partial(&vote)
        {
            return Ok(no_effects());
        }
        let storage: &dyn ReadStorage<TestScheme, TestCommand> = ctx.storage();
        if vote.view() > storage.get_cur_view()? {
            // The driver's keeper will replay this vote once the node reaches its view.
            return ctx.apply_as_honest(DriverInput::NewVote(vote));
        }

        // Each block gets its own quorum, signer deduplication, and ready flag.
        let collector = collectors
            .entry((vote.view(), vote.block_hash()))
            .or_insert_with(|| Collector::new(notary));
        if let CollectorOutput::GenericOutput(CertificateOutput::Ready(qc)) =
            collector.apply(CollectorInput::GenericVoteInput(vote))
        {
            // Match the state machine's Replay(NewQC) ordering.
            ctx.replica.push_front_input(DriverInput::NewQC(qc));
        }
        Ok(no_effects())
    }
}

pub fn equivocation<CS: ClockSource>()
-> impl FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> {
    |ctx, input| -> Result<NodeEffects<CS>> {
        let effects = ctx.apply_as_honest(input)?;
        let storage: &dyn ReadStorage<TestScheme, TestCommand> = ctx.storage();
        let cur_view = storage.get_cur_view()?;
        // Proposals can also be produced by replayed QCs, not just timer inputs.
        let Some(block_a) = effects.net.iter().find_map(|envelope| match &envelope.msg {
            Message::ProposalMsg(proposal) if proposal.view == cur_view => {
                Some(proposal.block.clone())
            }
            _ => None,
        }) else {
            return Ok(effects);
        };
        let notary = Notary::new(Arc::new(TestScheme {
            n: ctx.state().committee_size(),
            id: None,
        }));
        let (cmd_b, hash_b) = loop {
            let cmd = TestCommand(ctx.randomness.rand());
            let hash = notary.block_hash(&block_a.parent, &cmd, &block_a.justify, block_a.height);
            if hash != block_a.hash {
                break (cmd, hash);
            }
        };
        let block_b = Block {
            parent: block_a.parent,
            cmd: cmd_b,
            justify: block_a.justify.clone(),
            height: block_a.height,
            hash: hash_b,
        };

        let Effects {
            net,
            timers,
            sync,
            committed,
        } = effects;
        let mut effects = Effects {
            net: net.into_iter().filter(|envelope| {
                !matches!(&envelope.msg, Message::ProposalMsg(proposal) if proposal.view == cur_view)
            }).collect(),
            timers,
            sync,
            committed,
        };
        for id in (0..ctx.state().committee_size()).map(NodeID::from) {
            let (mut send_a, mut send_b) = match ctx.randomness.range(0, 2) {
                0 => (true, false),
                1 => (false, true),
                _ => (true, true),
            };

            // one test only
            send_a = true;
            send_b = true;
            if send_a {
                effects.net.push(Envelope {
                    to: Recipient::Node(id),
                    msg: Message::ProposalMsg(Proposal {
                        block: block_a.clone(),
                        view: cur_view,
                    }),
                });
            }
            if send_b {
                effects.net.push(Envelope {
                    to: Recipient::Node(id),
                    msg: Message::ProposalMsg(Proposal {
                        block: block_b.clone(),
                        view: cur_view,
                    }),
                });
            }
        }
        Ok(effects)
    }
}

pub fn tail_fork<CS: ClockSource>()
-> impl FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> {
    |ctx, input| {
        let mut effects = ctx.apply_as_honest(input)?;
        let storage: &dyn ReadStorage<TestScheme, TestCommand> = ctx.storage();
        let lock = storage.get_locked_qc()?;
        for envelope in &mut effects.net {
            let Message::ProposalMsg(proposal) = &mut envelope.msg else {
                continue;
            };
            if proposal.block.justify.view() <= lock.view() {
                continue;
            }
            let parent = storage
                .get_block_by_hash(&lock.block())?
                .context("the locked block is not stored")?;
            let height = parent.height + 1;
            let cmd = proposal.block.cmd.clone();
            let notary = Notary::new(Arc::new(TestScheme {
                n: ctx.state().committee_size(),
                id: None,
            }));
            let hash = notary.block_hash(&parent.hash, &cmd, &lock, height);
            proposal.block = Block {
                parent: parent.hash,
                cmd,
                justify: lock.clone(),
                height,
                hash,
            };
        }
        Ok(effects)
    }
}

/// Reveal the QC for `view - 1` to one recipient when proposing `view`, then remain silent.
pub fn reveal_qc_to_one<CS: ClockSource>(
    view: View,
    recipient: NodeID,
) -> impl FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> {
    let mut revealed = false;
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
    }
}

/// Given G -> B -> C, withhold QC(C) and propose a sibling of B on genesis at `attack_view`.
/// Report the protected and conflicting block hashes, then remain silent.
pub fn conflicting_proposal<CS: ClockSource>(
    attack_view: View,
    mut record_attack: impl FnMut(TestDigest, TestDigest),
) -> impl FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> {
    let mut sent_attack = false;
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
    }
}
