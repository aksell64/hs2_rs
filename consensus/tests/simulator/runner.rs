use crate::simulator::adversary::{Adversary, DeliveryPolicy, StableDelay};
use crate::simulator::simulator::{Context, GlobalState, ManualClock, NodeStorage, Simulator};
use anyhow::{Context as _, Result, bail, ensure};
use hotstuff_types::node::NodeID;
use std::fmt::Debug;

type Invariant<CS> = Box<dyn FnMut(&Context<'_, CS>) -> Result<()>>;
type Stop<CS> = Box<dyn FnMut(&Context<'_, CS>) -> bool>;

pub struct Env<CS: ManualClock, DP: DeliveryPolicy<CS>> {
    adversary: Adversary<CS, DP>,
    invariants: Vec<Invariant<CS>>,
    stop: Stop<CS>,
    deadline: Option<CS::Clock>,
}

impl<CS: ManualClock> Env<CS, StableDelay> {
    pub fn new() -> Self {
        Self {
            adversary: Adversary::new([]),
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

    pub fn stop_node(&mut self, node_id: NodeID) -> Result<NodeStorage> {
        self.sim.stop_node(node_id)
    }

    pub fn recover_node<DP: DeliveryPolicy<CS>>(
        &mut self,
        node_id: NodeID,
        storage: NodeStorage,
        env: &mut Env<CS, DP>,
    ) -> Result<()> {
        self.sim
            .recover_node(node_id, storage, &mut env.adversary.delivery)?;
        self.check(env)
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
        self.run_inner(env, None)
    }

    pub fn run_until<DP: DeliveryPolicy<CS>>(
        &mut self,
        at: CS::Clock,
        env: &mut Env<CS, DP>,
    ) -> Result<()> {
        ensure!(at >= self.state().now(), "cannot run backwards to {at:?}");
        self.run_inner(env, Some(at))
    }

    fn run_inner<DP: DeliveryPolicy<CS>>(
        &mut self,
        env: &mut Env<CS, DP>,
        until: Option<CS::Clock>,
    ) -> Result<()> {
        let committee_size = self.sim.state().committee_size();
        env.adversary
            .prepare_behaviors(committee_size, &self.sim.rng)?;
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
        loop {
            if self.maybe_stop(env, until)? {
                break;
            }
            self.sim.tick(&mut env.adversary)?;
            self.check(env)?;
        }
        Ok(())
    }

    fn maybe_stop<DP: DeliveryPolicy<CS>>(
        &mut self,
        env: &mut Env<CS, DP>,
        until: Option<CS::Clock>,
    ) -> Result<bool> {
        if let Some(at) = until {
            if self.state().next_time().is_none_or(|next| next > at) {
                self.sim.advance_to(at);
                self.check(env)?;
                return Ok(true);
            }
        } else if (env.stop)(&self.sim.context(&env.adversary.byzantine_nodes)) {
            return Ok(true);
        }
        let now = self.sim.state().now();
        match (self.sim.state().next_time(), env.deadline) {
            (None, _) => bail!("stalled at {now:?}: no events, no timers"),
            (Some(at), Some(deadline)) if at > deadline => {
                bail!("condition not reached by {deadline:?}, now {now:?}")
            }
            _ => {}
        }
        Ok(false)
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
