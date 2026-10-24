use crate::simulator::adversary::{Adversary, DeliveryPolicy, StableDelay};
use crate::simulator::dsl::{VirtualClock, all_committed, min_committed_height, time_reached};
use crate::simulator::simulator::{GlobalState, ManualClock, Simulator};
use anyhow::{Context as _, Result, bail};
use hotstuff_types::node::NodeID;
use std::fmt::Debug;
use std::mem;

type Invariant<CS> = Box<dyn FnMut(&GlobalState<CS>) -> Result<()>>;
type Stop<CS> = Box<dyn FnMut(&GlobalState<CS>) -> bool>;

pub struct Runner<CS: ManualClock, DP: DeliveryPolicy<CS>> {
    sim: Simulator<CS>,
    adversary: Adversary<CS, DP>,
    invariants: Vec<Invariant<CS>>,
    stop: Stop<CS>,
    deadline: Option<CS::Clock>,
}

fn never<CS: ManualClock>() -> Stop<CS> {
    Box::new(|_| false)
}

impl<CS: ManualClock> Runner<CS, StableDelay>
where
    CS::Clock: Debug,
{
    pub fn new(sim: Simulator<CS>) -> Self {
        Self {
            sim,
            adversary: Adversary::new(),
            invariants: Vec::new(),
            stop: never(),
            deadline: None,
        }
    }
}

impl<CS: ManualClock, DP: DeliveryPolicy<CS>> Runner<CS, DP>
where
    CS::Clock: Debug,
{
    pub fn adversary<P: DeliveryPolicy<CS>>(self, adversary: Adversary<CS, P>) -> Runner<CS, P> {
        Runner {
            sim: self.sim,
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

    pub fn crash(mut self, node_id: NodeID) -> Self {
        self.sim.crash(node_id);
        self
    }

    pub fn state(&self) -> &GlobalState<CS> {
        self.sim.state()
    }

    pub fn run(mut self) -> Result<Self> {
        let mut stop = mem::replace(&mut self.stop, never());
        let deadline = self.deadline.take();
        self.check()?;
        while !stop(self.sim.state()) {
            let now = self.sim.state().now();
            match (self.sim.state().next_time(), deadline) {
                (None, _) => bail!("stalled at {now:?}: no events, no timers"),
                (Some(at), Some(deadline)) if at > deadline => {
                    bail!("condition not reached by {deadline:?}, now {now:?}")
                }
                _ => {}
            }
            self.sim.tick(&mut self.adversary)?;
            self.check()?;
        }
        Ok(self)
    }

    fn check(&mut self) -> Result<()> {
        let state = self.sim.state();
        for invariant in &mut self.invariants {
            invariant(state).with_context(|| format!("invariant violated at {:?}", state.now()))?;
        }
        Ok(())
    }
}

impl<DP: DeliveryPolicy<VirtualClock>> Runner<VirtualClock, DP> {
    pub fn progress_after(self, gst: u64, blocks: u64, within: u64) -> Result<Self> {
        let runner = self.stop_cond(time_reached(gst)).run()?;
        let height = min_committed_height(runner.state());
        runner
            .stop_cond(all_committed(height + blocks))
            .deadline(gst + within)
            .run()
    }
}
