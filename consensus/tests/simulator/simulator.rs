use crate::simulator::clock::Timeline;
use anyhow::{Context, Result, anyhow};
use codec::{DecodeAll, Encode};
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::mempool::TestMempool;
use hotstuff_consensus::mocks::scheme::{TestCommand, TestDigest, TestScheme};
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::network::message::{Message, Recipient};
use hotstuff_consensus::protocol::driver::DriverInput;
use hotstuff_consensus::protocol::leaders::RoundRobin;
use hotstuff_consensus::protocol::pacemaker::Pacemaker;
use hotstuff_consensus::sm::{Effects, NodeError, NodeState};
use hotstuff_consensus::storage::ReadStorage;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::timer::queue::TimerQueue;
use hotstuff_consensus::timer::service::{TimerCommand, TimerEvent};
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::block::Block;
use hotstuff_consensus::types::clock::{Clock, ClockSource};
use hotstuff_types::node::NodeID;
use std::collections::BTreeMap;
use std::marker::PhantomData;

pub type Replica<CS> = NodeState<
    TestScheme,
    TestCommand,
    KVStorage<MemoryKV<Vec<u8>, Vec<u8>>>,
    TestMempool,
    FixedPolicy<<CS as ClockSource>::Clock>,
    CS,
    RoundRobin,
>;

pub enum Event {
    Deliver {
        from: NodeID,
        to: NodeID,
        bytes: Vec<u8>,
    },
    Synced {
        node_id: NodeID,
        blocks: Vec<Vec<u8>>,
    },
    Timer {
        node_id: NodeID,
        timer: TimerEvent,
    },
}

pub trait ManualClock: ClockSource + Clone {
    fn set(&self, now: Self::Clock);
}

pub struct Commit<CL> {
    pub at: CL,
    pub height: u64,
    pub hash: TestDigest,
}

pub struct GlobalState<CS: ClockSource> {
    nodes: BTreeMap<NodeID, Replica<CS>>,
    timeline: Timeline<CS::Clock>,
    timers: BTreeMap<NodeID, TimerQueue<CS::Clock>>,
    stable_net_delay: <<CS as ClockSource>::Clock as Clock>::Duration,
    clock: CS,
    commits: BTreeMap<NodeID, Vec<Commit<CS::Clock>>>,
    view_timeouts: u64,
}

impl<CS: ClockSource> GlobalState<CS> {
    pub fn now(&self) -> CS::Clock {
        self.timeline.now()
    }

    pub fn running(&self) -> impl Iterator<Item = NodeID> + '_ {
        self.nodes.keys().copied()
    }

    pub fn commit_logs(&self) -> impl Iterator<Item = (NodeID, &[Commit<CS::Clock>])> {
        self.commits.iter().map(|(id, log)| (*id, log.as_slice()))
    }

    pub fn committed_height(&self, node_id: NodeID) -> u64 {
        self.commits
            .get(&node_id)
            .and_then(|log| log.last())
            .map_or(0, |commit| commit.height)
    }

    pub fn view_timeouts(&self) -> u64 {
        self.view_timeouts
    }
}

pub type Duration<CS> = <<CS as ClockSource>::Clock as Clock>::Duration;

pub struct Simulator<CS: ClockSource> {
    state: GlobalState<CS>,
}

impl<CS: ManualClock> Simulator<CS> {
    pub fn new(
        clock: CS,
        nodes: BTreeMap<NodeID, Replica<CS>>,
        stable_net_delay: Duration<CS>,
    ) -> Result<Self> {
        let timers = nodes.keys().map(|id| (*id, TimerQueue::new())).collect();
        let mut sim = Self {
            state: GlobalState {
                timeline: Timeline::new(clock.now()),
                nodes,
                timers,
                stable_net_delay,
                clock,
                commits: BTreeMap::new(),
                view_timeouts: 0,
            },
        };
        let stable = FixedDelayDeliveryPolicyBuilder::new();
        let now = sim.state.now();
        let ids: Vec<NodeID> = sim.state.running().collect();
        for node_id in ids {
            let effects = sim.state.nodes.get_mut(&node_id).context("node is not running")?.apply()?;
            let mut policy = stable.policy_from_state(&sim.state);
            sim.execute_effects(now, node_id, effects, &mut policy)?;
        }
        Ok(sim)
    }

    pub fn state(&self) -> &GlobalState<CS> {
        &self.state
    }

    pub fn crash(&mut self, node_id: NodeID) {
        self.state.nodes.remove(&node_id);
        self.state.timers.remove(&node_id);
    }
}

impl<CS: ManualClock> Simulator<CS> {
    fn next_event(&mut self) -> Option<Event> {
        match (self.next_timer_deadline(), self.state.timeline.next_time()) {
            (Some(deadline), next) if next.is_none_or(|at| deadline < at) => {
                self.state.timeline.advance_to(deadline);
                let (node_id, timer) = self
                    .pop_due_timer(deadline)
                    .expect("a timer is due at the earliest deadline");
                Some(Event::Timer { node_id, timer })
            }
            _ => self.state.timeline.fire_next_event(),
        }
    }

    fn next_timer_deadline(&self) -> Option<CS::Clock> {
        self.state
            .timers
            .values()
            .filter_map(TimerQueue::next_deadline)
            .min()
    }

    fn pop_due_timer(&mut self, now: CS::Clock) -> Option<(NodeID, TimerEvent)> {
        self.state
            .timers
            .iter_mut()
            .find_map(|(id, timers)| timers.pop_due(now).map(|timer| (*id, timer)))
    }

    pub fn next_time(&self) -> Option<CS::Clock> {
        self.next_timer_deadline()
            .into_iter()
            .chain(self.state.timeline.next_time())
            .min()
    }

    pub fn tick<DPB: DeliveryPolicyBuilder<CS>>(
        &mut self,
        delivery_builder: &mut DPB,
    ) -> Result<bool> {
        let Some((node_id, now, effects)) = self.advance_next_time()? else {
            return Ok(false);
        };
        let mut policy = delivery_builder.policy_from_state(&self.state);
        self.execute_effects(now, node_id, effects, &mut policy)?;
        Ok(true)
    }

    fn advance_next_time(
        &mut self,
    ) -> Result<
        Option<(
            NodeID,
            CS::Clock,
            Effects<TestScheme, TestCommand, CS::Clock>,
        )>,
    > {
        let Some(event) = self.next_event() else {
            return Ok(None);
        };
        let now = self.state.timeline.now();
        self.state.clock.set(now);

        let (node_id, inputs) = match event {
            Event::Deliver { from, to, bytes } => {
                let msg = Message::<TestScheme, TestCommand>::decode_all(&mut &bytes[..])
                    .map_err(|err| anyhow!("undecodable message from {from:?}: {err:?}"))?;
                (to, vec![msg.into_event(from)])
            }
            Event::Synced { node_id, blocks } => {
                let mut inputs = Vec::with_capacity(blocks.len());
                for bytes in blocks {
                    let block = Block::<TestScheme, TestCommand>::decode_all(&mut &bytes[..])
                        .map_err(|err| anyhow!("undecodable block: {err:?}"))?;
                    inputs.push(DriverInput::BlockSynced(block));
                }
                (node_id, inputs)
            }
            Event::Timer { node_id, timer } => {
                let input = match timer {
                    TimerEvent::ViewTimeout(view) => {
                        self.state.view_timeouts += 1;
                        DriverInput::ViewTimerFired(view)
                    }
                    TimerEvent::LockTimeout(view) => DriverInput::LockTimerFired(view),
                };
                (node_id, vec![input])
            }
        };

        let effects = match self.state.nodes.get_mut(&node_id) {
            Some(replica) => {
                for input in inputs {
                    replica.add_event_to_queue(input);
                }
                replica.apply()?
            }
            None => Effects {
                net: Vec::new(),
                timers: Vec::new(),
                sync: Vec::new(),
                committed: Vec::new(),
            },
        };
        Ok(Some((node_id, now, effects)))
    }

    fn execute_effects<DP: DeliveryPolicy<CS>>(
        &mut self,
        now: CS::Clock,
        node_id: NodeID,
        effects: Effects<TestScheme, TestCommand, CS::Clock>,
        policy: &mut DP,
    ) -> Result<()> {
        for envelope in effects.net {
            let bytes = envelope.msg.encode();
            let recipients: Vec<NodeID> = match envelope.to {
                Recipient::All => self.state.nodes.keys().copied().collect(),
                Recipient::Node(to) => vec![to],
            };
            for to in recipients {
                let at = if to == node_id {
                    Some(now)
                } else {
                    let packet = Packet {
                        to,
                        payload: Payload::Message {
                            from: node_id,
                            msg: &envelope.msg,
                        },
                    };
                    self.route(policy, now, &packet)
                };
                if let Some(at) = at {
                    let event = Event::Deliver {
                        from: node_id,
                        to,
                        bytes: bytes.clone(),
                    };
                    self.state.timeline.add_event(event, at);
                }
            }
        }

        let timers = self.state.timers.entry(node_id).or_default();
        for command in effects.timers {
            match command {
                TimerCommand::SetEpochTimers(epoch) => timers.set_epoch_timers(epoch),
                TimerCommand::SetLockTimer(timeout) => timers.set_lock_timer(timeout),
                TimerCommand::ClearLockTimer(view) => timers.stop_lock_timer(view),
                TimerCommand::ClearViewTimersBelow(view) => timers.stop_view_timers_below(view),
            }
        }

        for digest in effects.sync {
            let blocks = self.missing_branch(node_id, digest);
            if blocks.is_empty() {
                continue;
            }
            let packet = Packet {
                to: node_id,
                payload: Payload::Sync { digest: &digest },
            };
            if let Some(at) = self.route(policy, now, &packet) {
                self.state
                    .timeline
                    .add_event(Event::Synced { node_id, blocks }, at);
            }
        }

        if !effects.committed.is_empty() {
            let log = self.state.commits.entry(node_id).or_default();
            for block in effects.committed {
                log.push(Commit {
                    at: now,
                    height: block.height,
                    hash: block.hash,
                });
            }
        }
        Ok(())
    }

    fn route<DP: DeliveryPolicy<CS>>(
        &self,
        policy: &mut DP,
        now: CS::Clock,
        packet: &Packet,
    ) -> Option<CS::Clock> {
        let context = DeliveryContext {
            now,
            state: &self.state,
        };
        match policy.route(&context, packet) {
            Fate::Delay(delay) => Some(now.add(delay)),
            Fate::Drop => None,
        }
    }

    fn missing_branch(&self, node_id: NodeID, digest: TestDigest) -> Vec<Vec<u8>> {
        let lookup = |id: &NodeID, hash: &TestDigest| {
            ReadStorage::<TestScheme, TestCommand>::get_block_by_hash(
                self.state.nodes[id].storage(),
                hash,
            )
            .expect("in-memory storage read failed")
        };
        let mut branch = Vec::new();
        let mut cursor = digest;
        while lookup(&node_id, &cursor).is_none() {
            let Some(block) = self.state.nodes.keys().find_map(|id| lookup(id, &cursor)) else {
                break;
            };
            cursor = block.parent;
            branch.push(block.encode());
        }
        branch.reverse();
        branch
    }
}

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

pub trait DeliveryPolicyBuilder<CS: ClockSource> {
    type Policy: DeliveryPolicy<CS>;

    fn policy_from_state(&self, state: &GlobalState<CS>) -> Self::Policy;
}

impl<CS, F, DP> DeliveryPolicyBuilder<CS> for F
where
    CS: ClockSource,
    F: Fn(&GlobalState<CS>) -> DP,
    DP: DeliveryPolicy<CS>,
{
    type Policy = DP;

    fn policy_from_state(&self, state: &GlobalState<CS>) -> DP {
        self(state)
    }
}

pub struct FixedDelayDeliveryPolicyBuilder<CS: ClockSource> {
    _phantom: PhantomData<CS>,
}

impl<CS: ClockSource> FixedDelayDeliveryPolicyBuilder<CS> {
    pub fn new() -> Self {
        Self {
            _phantom: PhantomData,
        }
    }
}

impl<CS: ClockSource> DeliveryPolicyBuilder<CS> for FixedDelayDeliveryPolicyBuilder<CS> {
    type Policy = FixedDelay<Duration<CS>>;

    fn policy_from_state(&self, state: &GlobalState<CS>) -> Self::Policy {
        FixedDelay {
            delay: state.stable_net_delay.clone(),
        }
    }
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
