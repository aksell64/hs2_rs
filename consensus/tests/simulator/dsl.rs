use crate::simulator::rng::{Randomness, SplitMix64};
use crate::simulator::simulator::{Context, GlobalState, ManualClock, Simulator};
use crate::simulator::trace::{Trace, node, record_violation, with_name};
use anyhow::{Context as _, Result, bail};
use hotstuff_consensus::mocks::genesis::genesis_config;
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::mempool::TestMempool;
use hotstuff_consensus::mocks::scheme::{TestCommand, TestScheme};
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::protocol::leaders::RoundRobin;
use hotstuff_consensus::sm::NodeState;
use hotstuff_consensus::storage::ReadStorage;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_consensus::types::crypto::Notary;
use hotstuff_types::node::NodeID;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub type Sim = Simulator<VirtualClock>;
pub type State = GlobalState<VirtualClock>;

#[derive(Clone, Copy, Debug)]
pub enum NodeSelector {
    Id(NodeID),
    All,
    Range(u32),
}

pub fn with_probability<CS: ClockSource>(percent: u64) -> impl FnMut(&Context<'_, CS>) -> bool {
    assert!(
        percent <= 100,
        "probability must be between 0 and 100 percent"
    );
    move |ctx| ctx.randomness.chance(percent)
}

const COMMAND_SPACE: u64 = 1_000_000;

#[derive(Clone, Default)]
pub struct VirtualClock(Arc<AtomicU64>);

impl ClockSource for VirtualClock {
    type Clock = u64;

    fn now(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

impl ManualClock for VirtualClock {
    fn set(&self, now: u64) {
        self.0.store(now, Ordering::Relaxed);
    }
}

pub struct Setup {
    n: u32,
    crashed: BTreeSet<NodeID>,
    view_timeout: u64,
    lock_timeout: u64,
    delay: u64,
    rng: Randomness,
    verbose: bool,
    starts: BTreeMap<NodeID, u64>,
    stagger: Option<(u64, u64)>,
}

impl Setup {
    pub fn new(n: u32) -> Self {
        Self {
            n,
            crashed: BTreeSet::new(),
            view_timeout: 100,
            lock_timeout: 30,
            delay: 10,
            rng: Randomness::new(0),
            verbose: false,
            starts: BTreeMap::new(),
            stagger: None,
        }
    }

    pub fn crashed(mut self, ids: impl IntoIterator<Item = u32>) -> Self {
        self.crashed.extend(ids.into_iter().map(NodeID::from));
        self
    }

    pub fn timeouts(mut self, view_timeout: u64, lock_timeout: u64) -> Self {
        self.view_timeout = view_timeout;
        self.lock_timeout = lock_timeout;
        self
    }

    pub fn delay(mut self, delay: u64) -> Self {
        self.delay = delay;
        self
    }

    pub fn rng(mut self, rng: Randomness) -> Self {
        self.rng = rng;
        self
    }

    pub fn verbose(mut self, verbose: bool) -> Self {
        self.verbose = verbose;
        self
    }

    pub fn start_at(mut self, starts: impl IntoIterator<Item = (u32, u64)>) -> Self {
        self.starts
            .extend(starts.into_iter().map(|(id, at)| (NodeID::from(id), at)));
        self
    }

    pub fn start_in_interval(self, rng: &Randomness, start: u64, end: u64) -> Self {
        let n = self.n;
        self.start_at((0..n).map(|id| (id, rng.range(start, end))))
    }

    pub fn stagger(mut self, max: u64, seed: u64) -> Self {
        self.stagger = Some((max, seed));
        self
    }

    pub fn build(&self) -> Result<Sim> {
        let clock = VirtualClock::default();
        let mut nodes = BTreeMap::new();
        for id in (0..self.n).map(NodeID::from) {
            if self.crashed.contains(&id) {
                continue;
            }
            let replica = NodeState::new(
                Notary::new(Arc::new(TestScheme {
                    n: self.n,
                    id: Some(id),
                })),
                KVStorage::new(MemoryKV::new()),
                TestMempool::new(u64::from(u32::from(id)) * COMMAND_SPACE + 1),
                id,
                clock.clone(),
                FixedPolicy {
                    view_timeout: self.view_timeout,
                    lock_timeout: self.lock_timeout,
                },
                RoundRobin::new(self.n),
                genesis_config(self.n, "sim"),
            )?;
            nodes.insert(id, replica);
        }
        let mut starts = self.starts.clone();
        if let Some((max, seed)) = self.stagger {
            let mut rng = SplitMix64::new(seed);
            for id in nodes.keys() {
                starts.entry(*id).or_insert_with(|| rng.range(0, max));
            }
        }
        let trace = self
            .verbose
            .then(|| {
                Trace::create(
                    &self.describe(&starts),
                    self.n,
                    self.rng.seed(),
                    &self.crashed,
                )
            })
            .transpose()?;
        Ok(Simulator::new(
            clock,
            self.n,
            nodes,
            starts,
            self.delay,
            self.rng.clone(),
            trace,
        ))
    }

    fn describe(&self, starts: &BTreeMap<NodeID, u64>) -> String {
        let crashed: Vec<String> = self.crashed.iter().copied().map(node).collect();
        let starts: Vec<String> = (0..self.n)
            .map(NodeID::from)
            .filter(|id| !self.crashed.contains(id))
            .map(|id| format!("{}@{}", node(id), starts.get(&id).copied().unwrap_or(0)))
            .collect();
        format!(
            "n = {}, Δ = {}, view timeout = {}, lock timeout = {}, crashed = [{}], starts = [{}], seed = {}",
            self.n,
            self.delay,
            self.view_timeout,
            self.lock_timeout,
            crashed.join(", "),
            starts.join(", "),
            self.rng.seed()
        )
    }
}

pub fn min_committed_height(ctx: &Context<'_, VirtualClock>) -> u64 {
    ctx.honest_nodes()
        .map(|id| ctx.state.committed_height(id))
        .min()
        .unwrap_or(0)
}

pub fn max_committed_height(ctx: &Context<'_, VirtualClock>) -> u64 {
    ctx.honest_nodes()
        .map(|id| ctx.state.committed_height(id))
        .max()
        .unwrap_or(0)
}

pub fn committed_proposers(state: &State, id: NodeID) -> Result<BTreeSet<NodeID>> {
    let replica = state.replica(id).context("the node is not running")?;
    let storage: &dyn ReadStorage<TestScheme, TestCommand> = replica.storage();
    let mut proposers = BTreeSet::new();
    let mut hash = storage.get_exec_hash()?;
    while let Some(block) = storage.get_block_by_hash(&hash)? {
        if block.height == 0 {
            break;
        }
        proposers.insert(NodeID::from((block.cmd.0 / COMMAND_SPACE) as u32));
        hash = block.parent;
    }
    Ok(proposers)
}

pub fn min_view(ctx: &Context<'_, VirtualClock>) -> View {
    ctx.honest_views().map(|(_, view)| view).min().unwrap_or(0)
}

pub fn max_view(ctx: &Context<'_, VirtualClock>) -> View {
    ctx.honest_views().map(|(_, view)| view).max().unwrap_or(0)
}

pub fn time_reached(time: u64) -> impl Fn(&Context<'_, VirtualClock>) -> bool {
    move |ctx| ctx.state.next_time().is_none_or(|at| at > time)
}

pub fn random_seeds(count: usize) -> Vec<u64> {
    let mut rng = SplitMix64::new(0);
    (0..count).map(|_| rng.next_u64()).collect()
}

/// Pass the scenario function directly so its name is available for the trace.
pub fn replay(seed: u64, scenario: impl FnOnce(u64, bool) -> Result<()>) -> Result<()> {
    with_name(std::any::type_name_of_val(&scenario), || scenario(seed, true))
}

pub fn for_each_seed(seeds: &[u64], test: impl Fn(u64, bool) -> Result<()>) -> Result<()> {
    for &seed in seeds {
        if let Err(err) = test(seed, false) {
            let Err(rerun) = test(seed, true) else {
                bail!(
                    "seed {seed} failed, then passed when rerun with a trace: the simulation is not deterministic"
                );
            };
            record_violation(&format!("{rerun:#}"))?;
            return Err(err.context(format!("seed {seed}")));
        }
    }
    Ok(())
}
