use crate::simulator::simulator::{GlobalState, NodeEffects, Replica, no_effects};
use anyhow::{Context as _, Result};
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::scheme::{TestCommand, TestScheme};
use hotstuff_consensus::network::message::Message;
use hotstuff_consensus::protocol::driver::DriverInput;
use hotstuff_consensus::storage::ReadStorage;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::types::block::Block;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_consensus::types::crypto::Notary;
use hotstuff_types::node::NodeID;
use std::sync::Arc;

pub type Input = DriverInput<TestScheme, TestCommand>;
pub type Behavior<CS> =
    Box<dyn FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>>>;

pub struct ByzantineProxy<CS: ClockSource> {
    pub activate_when: Box<dyn FnMut(&GlobalState<CS>) -> bool>,
    pub behavior: Behavior<CS>,
}

pub struct ByzantineContext<'a, CS: ClockSource> {
    id: NodeID,
    replica: &'a mut Replica<CS>,
    state: &'a GlobalState<CS>,
}

impl<'a, CS: ClockSource> ByzantineContext<'a, CS> {
    pub fn new(id: NodeID, replica: &'a mut Replica<CS>, state: &'a GlobalState<CS>) -> Self {
        Self { id, replica, state }
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

pub fn tail_fork<CS: ClockSource>(
    committee_size: u32,
) -> impl FnMut(&mut ByzantineContext<'_, CS>, Input) -> Result<NodeEffects<CS>> {
    let notary = Notary::new(Arc::new(TestScheme {
        n: committee_size,
        id: None,
    }));
    move |ctx, input| {
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
