use crate::simulator::clock::TieBreak;
use crate::simulator::rng::SplitMix64;
use crate::simulator::simulator::{GlobalState, ManualClock, Simulator};
use crate::simulator::trace::{Trace, node};
use anyhow::Result;
use hotstuff_consensus::mocks::genesis::genesis_config;
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::mempool::TestMempool;
use hotstuff_consensus::mocks::scheme::TestScheme;
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::protocol::leaders::RoundRobin;
use hotstuff_consensus::sm::NodeState;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_consensus::types::crypto::Notary;
use hotstuff_types::node::NodeID;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub type Sim = Simulator<VirtualClock>;
pub type State = GlobalState<VirtualClock>;

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
    shuffle: Option<u64>,
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
            shuffle: None,
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

    pub fn shuffle(mut self, seed: u64) -> Self {
        self.shuffle = Some(seed);
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
        let tie_break = self
            .shuffle
            .map_or(TieBreak::Fifo, |seed| TieBreak::Shuffle(SplitMix64::new(seed)));
        let trace = self
            .verbose
            .then(|| Trace::create(&self.describe(&starts)))
            .transpose()?;
        Ok(Simulator::new(
            clock,
            nodes,
            starts,
            self.delay,
            tie_break,
            trace,
        ))
    }

    fn describe(&self, starts: &BTreeMap<NodeID, u64>) -> String {
        let crashed: Vec<String> = self.crashed.iter().copied().map(node).collect();
        let starts: Vec<String> = starts
            .iter()
            .filter(|(_, at)| **at > 0)
            .map(|(id, at)| format!("{}@{at}", node(*id)))
            .collect();
        let ties = self
            .shuffle
            .map_or("fifo".to_owned(), |seed| format!("shuffle (seed {seed})"));
        format!(
            "n = {}, Δ = {}, view timeout = {}, lock timeout = {}, crashed = [{}], late starts = [{}], ties = {ties}",
            self.n,
            self.delay,
            self.view_timeout,
            self.lock_timeout,
            crashed.join(", "),
            starts.join(", ")
        )
    }
}

pub fn min_committed_height(state: &State) -> u64 {
    state
        .running()
        .map(|id| state.committed_height(id))
        .min()
        .unwrap_or(0)
}

pub fn time_reached(time: u64) -> impl Fn(&State) -> bool {
    move |state| state.next_time().is_none_or(|at| at > time)
}

pub fn all_committed(height: u64) -> impl Fn(&State) -> bool {
    move |state| min_committed_height(state) >= height
}

pub fn and(a: impl Fn(&State) -> bool, b: impl Fn(&State) -> bool) -> impl Fn(&State) -> bool {
    move |state| a(state) && b(state)
}

pub fn or(a: impl Fn(&State) -> bool, b: impl Fn(&State) -> bool) -> impl Fn(&State) -> bool {
    move |state| a(state) || b(state)
}
