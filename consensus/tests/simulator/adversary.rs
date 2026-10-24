use crate::simulator::dsl::VirtualClock;
use crate::simulator::rng::SplitMix64;
use crate::simulator::simulator::{Duration, GlobalState};
use hotstuff_consensus::mocks::scheme::{TestCommand, TestDigest, TestScheme};
use hotstuff_consensus::network::message::Message;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_types::node::NodeID;
use std::collections::BTreeSet;
use std::marker::PhantomData;

pub struct Adversary<CS: ClockSource, D: DeliveryPolicy<CS>> {
    pub delivery: D,
    _phantom_clock: PhantomData<CS>,
}

impl<CS: ClockSource> Adversary<CS, StableDelay> {
    pub fn new() -> Self {
        Self {
            delivery: StableDelay,
            _phantom_clock: PhantomData,
        }
    }
}

impl<CS: ClockSource, D: DeliveryPolicy<CS>> Adversary<CS, D> {
    pub fn delivery<P: DeliveryPolicy<CS>>(self, delivery: P) -> Adversary<CS, P> {
        Adversary {
            delivery,
            _phantom_clock: PhantomData,
        }
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
