use crate::simulator::simulator::{
    DeliveryContext, DeliveryPolicy, DeliveryPolicyBuilder, Duration, Fate, FixedDelay,
    GlobalState, ManualClock, Packet, Payload, Simulator,
};
use anyhow::{Context as _, Result, bail, ensure};
use hotstuff_consensus::mocks::genesis::genesis_config;
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::mempool::TestMempool;
use hotstuff_consensus::mocks::scheme::{TestDigest, TestScheme};
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::network::message::Message;
use hotstuff_consensus::protocol::leaders::RoundRobin;
use hotstuff_consensus::sm::NodeState;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_consensus::types::crypto::Notary;
use hotstuff_types::node::NodeID;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

pub type Sim = Simulator<VirtualClock>;
pub type State = GlobalState<VirtualClock>;
pub type Ctx<'a> = DeliveryContext<'a, VirtualClock>;

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

pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn range(&mut self, lo: u64, hi: u64) -> u64 {
        assert!(lo <= hi, "empty range {lo}..={hi}");
        match (hi - lo).checked_add(1) {
            Some(width) => lo + self.next_u64() % width,
            None => self.next_u64(),
        }
    }

    pub fn chance(&mut self, percent: u64) -> bool {
        self.range(0, 99) < percent
    }
}

pub struct Setup {
    n: u32,
    crashed: BTreeSet<NodeID>,
    view_timeout: u64,
    lock_timeout: u64,
    delay: u64,
}

impl Setup {
    pub fn new(n: u32) -> Self {
        Self {
            n,
            crashed: BTreeSet::new(),
            view_timeout: 100,
            lock_timeout: 30,
            delay: 10,
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
        Simulator::new(clock, nodes, self.delay)
    }
}

pub fn eventually<CS, DPB, F, I>(
    sim: &mut Simulator<CS>,
    delivery: &mut DPB,
    stop: F,
    invariant: &mut I,
    deadline: CS::Clock,
) -> Result<()>
where
    CS: ManualClock,
    CS::Clock: Debug,
    DPB: DeliveryPolicyBuilder<CS>,
    F: Fn(&GlobalState<CS>) -> bool,
    I: FnMut(&GlobalState<CS>) -> Result<()>,
{
    check(sim, invariant)?;
    while !stop(sim.state()) {
        match sim.next_time() {
            Some(at) if at <= deadline => {}
            Some(_) => bail!(
                "condition not reached by {deadline:?}, now {:?}",
                sim.state().now()
            ),
            None => bail!("stalled at {:?}: no events, no timers", sim.state().now()),
        }
        sim.tick(delivery)?;
        check(sim, invariant)?;
    }
    Ok(())
}

pub fn run_until<CS, DPB, I>(
    sim: &mut Simulator<CS>,
    delivery: &mut DPB,
    invariant: &mut I,
    until: CS::Clock,
) -> Result<()>
where
    CS: ManualClock,
    CS::Clock: Debug,
    DPB: DeliveryPolicyBuilder<CS>,
    I: FnMut(&GlobalState<CS>) -> Result<()>,
{
    check(sim, invariant)?;
    while sim.next_time().is_some_and(|at| at <= until) {
        sim.tick(delivery)?;
        check(sim, invariant)?;
    }
    Ok(())
}

fn check<CS, I>(sim: &Simulator<CS>, invariant: &mut I) -> Result<()>
where
    CS: ManualClock,
    CS::Clock: Debug,
    I: FnMut(&GlobalState<CS>) -> Result<()>,
{
    invariant(sim.state())
        .with_context(|| format!("invariant violated at {:?}", sim.state().now()))
}

pub fn min_committed_height(state: &State) -> u64 {
    state
        .running()
        .map(|id| state.committed_height(id))
        .min()
        .unwrap_or(0)
}

pub fn time_reached(time: u64) -> impl Fn(&State) -> bool {
    move |state| state.now() >= time
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

pub fn safety() -> impl FnMut(&State) -> Result<()> {
    let mut canonical: BTreeMap<u64, TestDigest> = BTreeMap::new();
    let mut checked: BTreeMap<NodeID, usize> = BTreeMap::new();
    move |state| {
        for (id, log) in state.commit_logs() {
            let done = checked.entry(id).or_insert(0);
            for i in *done..log.len() {
                let commit = &log[i];
                if let Some(prev) = i.checked_sub(1).map(|j| &log[j]) {
                    ensure!(
                        commit.height == prev.height + 1,
                        "{id:?} committed height {} right after {}",
                        commit.height,
                        prev.height
                    );
                }
                let hash = *canonical.entry(commit.height).or_insert(commit.hash);
                ensure!(
                    hash == commit.hash,
                    "{id:?} committed {:?} at height {}, but {hash:?} was committed there before",
                    commit.hash,
                    commit.height
                );
            }
            *done = log.len();
        }
        Ok(())
    }
}

pub fn no_view_timeouts() -> impl FnMut(&State) -> Result<()> {
    |state| {
        ensure!(
            state.view_timeouts() == 0,
            "{} view timers fired",
            state.view_timeouts()
        );
        Ok(())
    }
}

pub fn both(
    mut a: impl FnMut(&State) -> Result<()>,
    mut b: impl FnMut(&State) -> Result<()>,
) -> impl FnMut(&State) -> Result<()> {
    move |state| {
        a(state)?;
        b(state)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Proposal,
    Vote,
    Wish,
    Status,
    Tc,
    Sync,
}

impl Packet<'_> {
    pub fn from(&self) -> Option<NodeID> {
        match self.payload {
            Payload::Message { from, .. } => Some(from),
            Payload::Sync { .. } => None,
        }
    }

    pub fn kind(&self) -> Kind {
        match self.payload {
            Payload::Message { msg, .. } => match msg {
                Message::ProposalMsg(_) => Kind::Proposal,
                Message::VoteMsg(_) => Kind::Vote,
                Message::WishMsg(_) => Kind::Wish,
                Message::StatusMsg { .. } => Kind::Status,
                Message::TCMsg(_) => Kind::Tc,
            },
            Payload::Sync { .. } => Kind::Sync,
        }
    }

    pub fn view(&self) -> Option<View> {
        match self.payload {
            Payload::Message { msg, .. } => Some(match msg {
                Message::ProposalMsg(proposal) => proposal.view,
                Message::VoteMsg(vote) => vote.view(),
                Message::WishMsg(wish) => wish.view(),
                Message::StatusMsg { view, .. } => *view,
                Message::TCMsg(tc) => tc.view(),
            }),
            Payload::Sync { .. } => None,
        }
    }
}

pub struct Stateful<P>(Rc<RefCell<P>>);

pub fn stateful<P>(policy: P) -> Stateful<P> {
    Stateful(Rc::new(RefCell::new(policy)))
}

impl<CS: ClockSource, P: DeliveryPolicy<CS>> DeliveryPolicyBuilder<CS> for Stateful<P> {
    type Policy = Stateful<P>;

    fn policy_from_state(&self, _: &GlobalState<CS>) -> Stateful<P> {
        Stateful(Rc::clone(&self.0))
    }
}

impl<CS: ClockSource, P: DeliveryPolicy<CS>> DeliveryPolicy<CS> for Stateful<P> {
    fn route(&mut self, context: &DeliveryContext<CS>, packet: &Packet) -> Fate<Duration<CS>> {
        self.0.borrow_mut().route(context, packet)
    }
}

pub fn fixed(delay: u64) -> FixedDelay<u64> {
    FixedDelay { delay }
}

pub struct Uniform {
    lo: u64,
    hi: u64,
    rng: SplitMix64,
}

pub fn uniform(lo: u64, hi: u64, seed: u64) -> Uniform {
    Uniform {
        lo,
        hi,
        rng: SplitMix64::new(seed),
    }
}

impl DeliveryPolicy<VirtualClock> for Uniform {
    fn route(&mut self, _: &Ctx, _: &Packet) -> Fate<u64> {
        Fate::Delay(self.rng.range(self.lo, self.hi))
    }
}

pub struct UntilGst<P> {
    gst: u64,
    delta: u64,
    before: P,
}

pub fn until_gst<P>(gst: u64, delta: u64, before: P) -> UntilGst<P> {
    UntilGst { gst, delta, before }
}

impl<P: DeliveryPolicy<VirtualClock>> DeliveryPolicy<VirtualClock> for UntilGst<P> {
    fn route(&mut self, context: &Ctx, packet: &Packet) -> Fate<u64> {
        if context.now >= self.gst {
            return Fate::Delay(self.delta);
        }
        let cap = self.gst + self.delta - context.now;
        match self.before.route(context, packet) {
            Fate::Delay(delay) => Fate::Delay(delay.min(cap)),
            Fate::Drop => Fate::Delay(cap),
        }
    }
}

pub struct Partition {
    groups: Vec<BTreeSet<NodeID>>,
    heal: u64,
    delay: u64,
}

pub fn partition<const N: usize>(groups: [&[u32]; N], heal: u64, delay: u64) -> Partition {
    Partition {
        groups: groups
            .iter()
            .map(|group| group.iter().copied().map(NodeID::from).collect())
            .collect(),
        heal,
        delay,
    }
}

pub fn isolate(node: u32, heal: u64, delay: u64) -> Partition {
    partition([&[node]], heal, delay)
}

impl Partition {
    fn group_of(&self, id: NodeID) -> Option<usize> {
        self.groups.iter().position(|group| group.contains(&id))
    }
}

impl DeliveryPolicy<VirtualClock> for Partition {
    fn route(&mut self, context: &Ctx, packet: &Packet) -> Fate<u64> {
        let split = packet
            .from()
            .is_some_and(|from| self.group_of(from) != self.group_of(packet.to));
        if split && context.now < self.heal {
            Fate::Delay(self.heal - context.now + self.delay)
        } else {
            Fate::Delay(self.delay)
        }
    }
}

pub struct When<F, A, B> {
    pred: F,
    then: A,
    otherwise: B,
}

pub fn when<F, A, B>(pred: F, then: A, otherwise: B) -> When<F, A, B>
where
    F: FnMut(&Ctx, &Packet) -> bool,
{
    When {
        pred,
        then,
        otherwise,
    }
}

impl<F, A, B> DeliveryPolicy<VirtualClock> for When<F, A, B>
where
    F: FnMut(&Ctx, &Packet) -> bool,
    A: DeliveryPolicy<VirtualClock>,
    B: DeliveryPolicy<VirtualClock>,
{
    fn route(&mut self, context: &Ctx, packet: &Packet) -> Fate<u64> {
        if (self.pred)(context, packet) {
            self.then.route(context, packet)
        } else {
            self.otherwise.route(context, packet)
        }
    }
}

pub struct Lossy<P> {
    percent: u64,
    rng: SplitMix64,
    inner: P,
}

pub fn lossy<P>(percent: u64, seed: u64, inner: P) -> Lossy<P> {
    Lossy {
        percent,
        rng: SplitMix64::new(seed),
        inner,
    }
}

impl<P: DeliveryPolicy<VirtualClock>> DeliveryPolicy<VirtualClock> for Lossy<P> {
    fn route(&mut self, context: &Ctx, packet: &Packet) -> Fate<u64> {
        if self.rng.chance(self.percent) {
            Fate::Drop
        } else {
            self.inner.route(context, packet)
        }
    }
}
