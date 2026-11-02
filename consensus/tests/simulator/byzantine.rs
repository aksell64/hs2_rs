use crate::simulator::simulator::{GlobalState, NodeEffects, Replica, no_effects};
use anyhow::Result;
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::scheme::{TestCommand, TestScheme};
use hotstuff_consensus::protocol::driver::DriverInput;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_types::node::NodeID;

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
