use crate::simulator::rng::Randomness;
use crate::simulator::simulator::{Context, GlobalState, NodeEffects, Replica, no_effects};
use anyhow::Result;
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::scheme::{TestCommand, TestScheme};
use hotstuff_consensus::protocol::driver::{Driver, DriverInput};
use hotstuff_consensus::protocol::traits;
use hotstuff_consensus::sm::Effects;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_types::node::NodeID;
use std::cell::Cell;
use std::rc::Rc;

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

    pub fn force_apply<
        CO: traits::Consensus<TestScheme>,
        PM: traits::Pacemaker<TestScheme, Clock = CS::Clock>,
        CL: traits::Collector<TestScheme>,
    >(
        &mut self,
        input: Input,
        driver: &mut Driver<TestScheme, TestCommand, CO, PM, CL>,
    ) -> Result<NodeEffects<CS>> {
        Ok(self.replica.force_apply(input, driver)?)
    }

    pub fn inject(&mut self, input: Input) {
        self.replica.add_event_to_queue(input);
    }
}
