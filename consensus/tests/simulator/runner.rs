use crate::simulator::adversary::{Adversary, DeliveryPolicy, StableDelay};
use crate::simulator::simulator::{GlobalState, ManualClock, Simulator};
use anyhow::{Context as _, Result, bail};
use hotstuff_types::node::NodeID;
use std::fmt::Debug;

type Invariant<CS> = Box<dyn FnMut(&GlobalState<CS>) -> Result<()>>;
type Stop<CS> = Box<dyn FnMut(&GlobalState<CS>) -> bool>;

pub struct Env<CS: ManualClock, DP: DeliveryPolicy<CS>> {
    adversary: Adversary<CS, DP>,
    invariants: Vec<Invariant<CS>>,
    stop: Stop<CS>,
    deadline: Option<CS::Clock>,
}

impl<CS: ManualClock> Env<CS, StableDelay> {
    pub fn new() -> Self {
        Self {
            adversary: Adversary::new(),
            invariants: Vec::new(),
            stop: Box::new(|_| false),
            deadline: None,
        }
    }
}

impl<CS: ManualClock, DP: DeliveryPolicy<CS>> Env<CS, DP> {
    pub fn adversary<P: DeliveryPolicy<CS>>(self, adversary: Adversary<CS, P>) -> Env<CS, P> {
        Env {
            adversary,
            invariants: self.invariants,
            stop: self.stop,
            deadline: self.deadline,
        }
    }

    pub fn invariant(
        mut self,
        invariant: impl FnMut(&GlobalState<CS>) -> Result<()> + 'static,
    ) -> Self {
        self.invariants.push(Box::new(invariant));
        self
    }

    pub fn stop_cond(mut self, stop: impl FnMut(&GlobalState<CS>) -> bool + 'static) -> Self {
        self.stop = Box::new(stop);
        self
    }

    pub fn deadline(mut self, deadline: CS::Clock) -> Self {
        self.deadline = Some(deadline);
        self
    }
}

pub struct Runner<CS: ManualClock> {
    sim: Simulator<CS>,
}

impl<CS: ManualClock> Runner<CS>
where
    CS::Clock: Debug,
{
    pub fn new(sim: Simulator<CS>) -> Self {
        Self { sim }
    }

    pub fn crash(&mut self, node_id: NodeID) {
        self.sim.crash(node_id);
    }

    pub fn state(&self) -> &GlobalState<CS> {
        self.sim.state()
    }

    pub fn run<DP: DeliveryPolicy<CS>>(&mut self, mut env: Env<CS, DP>) -> Result<()> {
        self.check(&mut env.invariants)?;
        while !(env.stop)(self.sim.state()) {
            let now = self.sim.state().now();
            match (self.sim.state().next_time(), env.deadline) {
                (None, _) => bail!("stalled at {now:?}: no events, no timers"),
                (Some(at), Some(deadline)) if at > deadline => {
                    bail!("condition not reached by {deadline:?}, now {now:?}")
                }
                _ => {}
            }
            self.sim.tick(&mut env.adversary)?;
            self.check(&mut env.invariants)?;
        }
        Ok(())
    }

    fn check(&self, invariants: &mut [Invariant<CS>]) -> Result<()> {
        let state = self.sim.state();
        for invariant in invariants {
            invariant(state).with_context(|| format!("invariant violated at {:?}", state.now()))?;
        }
        Ok(())
    }
}
