use crate::simulator::adversary::{Adversary, DeliveryPolicy, StableDelay};
use crate::simulator::simulator::{Context, GlobalState, ManualClock, Simulator};
use anyhow::{Context as _, Result, bail, ensure};
use hotstuff_types::node::NodeID;
use std::fmt::Debug;

type Invariant<CS> = Box<dyn FnMut(&Context<'_, CS>) -> Result<()>>;
type Stop<CS> = Box<dyn FnMut(&Context<'_, CS>) -> bool>;

type Hook<CS> = Box<dyn FnMut(&Context<'_, CS>)>;

pub struct Env<CS: ManualClock, DP: DeliveryPolicy<CS>> {
    adversary: Adversary<CS, DP>,
    invariants: Vec<Invariant<CS>>,
    stop: Stop<CS>,
    deadline: Option<CS::Clock>,
    before_tick_hook: Option<Hook<CS>>,
    after_tick_hook: Option<Hook<CS>>,
}

impl<CS: ManualClock> Env<CS, StableDelay> {
    pub fn new() -> Self {
        Self {
            adversary: Adversary::new([]),
            invariants: Vec::new(),
            stop: Box::new(|_| false),
            deadline: None,
            before_tick_hook: None,
            after_tick_hook: None,
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
            before_tick_hook: self.before_tick_hook,
            after_tick_hook: self.after_tick_hook,
        }
    }

    pub fn invariant(
        mut self,
        invariant: impl FnMut(&Context<'_, CS>) -> Result<()> + 'static,
    ) -> Self {
        self.invariants.push(Box::new(invariant));
        self
    }

    /// Starts a new wait without a deadline. Call `deadline` afterwards to set one.
    pub fn stop_cond(mut self, stop: impl FnMut(&Context<'_, CS>) -> bool + 'static) -> Self {
        self.stop = Box::new(stop);
        self.deadline = None;
        self
    }

    pub fn deadline(mut self, deadline: CS::Clock) -> Self {
        self.deadline = Some(deadline);
        self
    }

    pub fn before_tick(mut self, hook: impl FnMut(&Context<'_, CS>) + 'static) -> Self {
        self.before_tick_hook = Some(Box::new(hook));
        self
    }

    pub fn after_tick(mut self, hook: impl FnMut(&Context<'_, CS>) + 'static) -> Self {
        self.after_tick_hook = Some(Box::new(hook));
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

    pub fn note(&mut self, note: String) {
        self.sim.note(note);
    }

    pub fn mark(&mut self, at: CS::Clock, label: &str) {
        self.sim.mark(at, label);
    }

    pub fn state(&self) -> &GlobalState<CS> {
        self.sim.state()
    }

    pub fn context<'a, DP: DeliveryPolicy<CS>>(&'a self, env: &'a Env<CS, DP>) -> Context<'a, CS> {
        self.sim.context(&env.adversary.byzantine_nodes)
    }

    /// Repeated runs of a scenario must declare the same Byzantine nodes.
    pub fn run<DP: DeliveryPolicy<CS>>(&mut self, env: &mut Env<CS, DP>) -> Result<()> {
        let committee_size = self.sim.state().committee_size();
        env.adversary.prepare_behaviors(committee_size, &self.sim.rng)?;
        for id in env.adversary.byzantine.keys() {
            ensure!(
                env.adversary.byzantine_nodes.contains(id),
                "{id:?} has a byzantine behavior but is not declared in Adversary::new"
            );
        }
        for id in env.adversary.mutations.keys() {
            ensure!(
                u32::from(*id) < committee_size && !env.adversary.byzantine_nodes.contains(id),
                "{id:?} has a mutation but is not a non-Byzantine committee member"
            );
        }
        self.sim.trace_behaviors(&env.adversary);
        self.check(env)?;
        while !(env.stop)(&self.sim.context(&env.adversary.byzantine_nodes)) {
            let now = self.sim.state().now();
            match (self.sim.state().next_time(), env.deadline) {
                (None, _) => bail!("stalled at {now:?}: no events, no timers"),
                (Some(at), Some(deadline)) if at > deadline => {
                    bail!("condition not reached by {deadline:?}, now {now:?}")
                }
                _ => {}
            }

            if let Some(hook) = &mut env.before_tick_hook {
                hook(&self.sim.context(&env.adversary.byzantine_nodes));
            }

            self.sim.tick(&mut env.adversary)?;

            if let Some(hook) = &mut env.after_tick_hook {
                hook(&self.sim.context(&env.adversary.byzantine_nodes));
            }

            self.check(env)?;
        }
        Ok(())
    }

    fn check<DP: DeliveryPolicy<CS>>(&self, env: &mut Env<CS, DP>) -> Result<()> {
        let context = self.sim.context(&env.adversary.byzantine_nodes);
        for invariant in &mut env.invariants {
            invariant(&context)
                .with_context(|| format!("invariant violated at {:?}", context.state.now()))?;
        }
        Ok(())
    }
}
