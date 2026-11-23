use crate::simulator::byzantine::ByzantineProxy;
use crate::simulator::dsl::{NodeSelector, VirtualClock};
use crate::simulator::rng::Randomness;
use crate::simulator::simulator::{Duration, GlobalState};
use anyhow::{Result, ensure};
use hotstuff_consensus::mocks::scheme::{TestCommand, TestDigest, TestScheme};
use hotstuff_consensus::network::message::Message;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_types::node::NodeID;
use std::collections::{BTreeMap, BTreeSet};

pub struct Adversary<CS: ClockSource, D: DeliveryPolicy<CS>> {
    pub delivery: D,
    pub byzantine_nodes: BTreeSet<NodeID>,
    pub byzantine: BTreeMap<NodeID, Vec<ByzantineProxy<'static, CS>>>,
    pub mutations: BTreeMap<NodeID, Vec<ByzantineProxy<'static, CS>>>,
    factories: Vec<BehaviorFactory<CS>>,
}

enum BehaviorKind {
    Byzantine,
    Mutated,
}

struct BehaviorFactory<CS: ClockSource> {
    kind: BehaviorKind,
    selector: NodeSelector,
    constructor: Box<dyn FnMut() -> ByzantineProxy<'static, CS>>,
}

impl<CS: ClockSource> Adversary<CS, StableDelay> {
    /// Declares faulty nodes even when they have no active Byzantine behavior.
    pub fn new(ids: impl IntoIterator<Item = u32>) -> Self {
        Self {
            delivery: StableDelay,
            byzantine_nodes: ids.into_iter().map(NodeID::from).collect(),
            byzantine: BTreeMap::new(),
            mutations: BTreeMap::new(),
            factories: Vec::new(),
        }
    }
}

impl<CS: ClockSource, D: DeliveryPolicy<CS>> Adversary<CS, D> {
    pub fn delivery<P: DeliveryPolicy<CS>>(self, delivery: P) -> Adversary<CS, P> {
        Adversary {
            delivery,
            byzantine_nodes: self.byzantine_nodes,
            byzantine: self.byzantine,
            mutations: self.mutations,
            factories: self.factories,
        }
    }

    /// Creates a separate proxy for each selected Byzantine node before the run starts.
    pub fn byzantine_node(
        mut self,
        selector: NodeSelector,
        constructor: impl FnMut() -> ByzantineProxy<'static, CS> + 'static,
    ) -> Self {
        self.factories.push(BehaviorFactory {
            kind: BehaviorKind::Byzantine,
            selector,
            constructor: Box::new(constructor),
        });
        self
    }

    /// Overrides selected non-Byzantine nodes without excluding them from honest-node checks.
    /// All and Range select from the committee, including nodes that have not started yet.
    pub fn mutate_node(
        mut self,
        selector: NodeSelector,
        constructor: impl FnMut() -> ByzantineProxy<'static, CS> + 'static,
    ) -> Self {
        self.factories.push(BehaviorFactory {
            kind: BehaviorKind::Mutated,
            selector,
            constructor: Box::new(constructor),
        });
        self
    }

    pub fn prepare_behaviors(&mut self, committee_size: u32, rng: &Randomness) -> Result<()> {
        if self.factories.is_empty() {
            return Ok(());
        }
        let byzantines: Vec<_> = self.byzantine_nodes.iter().copied().collect();
        let honest: Vec<_> = (0..committee_size)
            .map(NodeID::from)
            .filter(|id| !self.byzantine_nodes.contains(id))
            .collect();
        for mut factory in self.factories.drain(..) {
            let (candidates, handlers, method) = match factory.kind {
                BehaviorKind::Byzantine => (&byzantines, &mut self.byzantine, "byzantine_node"),
                BehaviorKind::Mutated => (&honest, &mut self.mutations, "mutate_node"),
            };
            let selected = match factory.selector {
                NodeSelector::Id(id) => {
                    ensure!(
                        candidates.contains(&id),
                        "{id:?} is not eligible for {method}"
                    );
                    vec![id]
                }
                NodeSelector::All => candidates.clone(),
                NodeSelector::Range(f) => rng
                    .sample_range(candidates.len() as u32, f)
                    .into_iter()
                    .map(|index| candidates[index as usize])
                    .collect(),
            };
            for id in selected {
                handlers
                    .entry(id)
                    .or_default()
                    .push((factory.constructor)());
            }
        }
        Ok(())
    }
}

pub type Ctx<'a> = DeliveryContext<'a, VirtualClock>;

pub enum Payload<'a> {
    Message {
        from: NodeID,
        msg: &'a Message<TestScheme, TestCommand>,
    },
    Sync {
        digest: &'a TestDigest,
    },
}

pub struct Packet<'a> {
    pub to: NodeID,
    pub payload: Payload<'a>,
}

pub enum Fate<D> {
    Delay(D),
    Drop,
}

pub struct DeliveryContext<'a, CS: ClockSource> {
    pub now: CS::Clock,
    pub state: &'a GlobalState<CS>,
}

pub trait DeliveryPolicy<CS: ClockSource> {
    fn route(&mut self, context: &DeliveryContext<CS>, packet: &Packet) -> Fate<Duration<CS>>;
}

impl<CS, F> DeliveryPolicy<CS> for F
where
    CS: ClockSource,
    F: FnMut(&DeliveryContext<CS>, &Packet) -> Fate<Duration<CS>>,
{
    fn route(&mut self, context: &DeliveryContext<CS>, packet: &Packet) -> Fate<Duration<CS>> {
        self(context, packet)
    }
}

pub struct FixedDelay<D> {
    pub delay: D,
}

impl<CS: ClockSource> DeliveryPolicy<CS> for FixedDelay<Duration<CS>> {
    fn route(&mut self, _: &DeliveryContext<CS>, _: &Packet) -> Fate<Duration<CS>> {
        Fate::Delay(self.delay)
    }
}

pub struct StableDelay;

impl<CS: ClockSource> DeliveryPolicy<CS> for StableDelay {
    fn route(&mut self, context: &DeliveryContext<CS>, _: &Packet) -> Fate<Duration<CS>> {
        Fate::Delay(context.state.stable_net_delay())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Proposal,
    Vote,
    Wish,
    Status,
    Tc,
    Qc,
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
                Message::QCMsg(_) => Kind::Qc,
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
                Message::QCMsg(qc) => qc.view(),
            }),
            Payload::Sync { .. } => None,
        }
    }
}

pub fn fixed(delay: u64) -> FixedDelay<u64> {
    FixedDelay { delay }
}

pub struct Uniform {
    lo: u64,
    hi: u64,
    rng: Randomness,
}

pub fn uniform(lo: u64, hi: u64, rng: Randomness) -> Uniform {
    Uniform { lo, hi, rng }
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

pub fn isolate_node_from_all(node: u32, heal: u64, delay: u64) -> Partition {
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
    rng: Randomness,
    inner: P,
}

pub fn lossy<P>(percent: u64, rng: Randomness, inner: P) -> Lossy<P> {
    Lossy {
        percent,
        rng,
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
