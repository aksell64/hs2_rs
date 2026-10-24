use crate::simulator::adversary::{
    Adversary, DeliveryContext, DeliveryPolicy, Fate, Packet, Payload,
};
use crate::simulator::clock::{TieBreak, Timeline};
use crate::simulator::trace::{Trace, describe, node};
use anyhow::{Result, anyhow};
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
use std::fmt::Debug;

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
    Start {
        node_id: NodeID,
    },
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
    dormant: BTreeMap<NodeID, Replica<CS>>,
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

    pub fn stable_net_delay(&self) -> Duration<CS> {
        self.stable_net_delay
    }

    pub fn next_time(&self) -> Option<CS::Clock> {
        self.timers
            .values()
            .filter_map(TimerQueue::next_deadline)
            .chain(self.timeline.next_time())
            .min()
    }
}

pub type Duration<CS> = <<CS as ClockSource>::Clock as Clock>::Duration;

pub struct Simulator<CS: ClockSource> {
    state: GlobalState<CS>,
    trace: Option<Trace>,
}

impl<CS: ManualClock> Simulator<CS>
where
    CS::Clock: Debug,
{
    pub fn new(
        clock: CS,
        nodes: BTreeMap<NodeID, Replica<CS>>,
        starts: BTreeMap<NodeID, CS::Clock>,
        stable_net_delay: Duration<CS>,
        tie_break: TieBreak,
        trace: Option<Trace>,
    ) -> Self {
        let mut timeline = Timeline::new(clock.now(), tie_break);
        for node_id in nodes.keys().copied() {
            let at = starts.get(&node_id).copied().unwrap_or(clock.now());
            timeline.add_event(Event::Start { node_id }, at);
        }
        Self {
            state: GlobalState {
                nodes: BTreeMap::new(),
                dormant: nodes,
                timeline,
                timers: BTreeMap::new(),
                stable_net_delay,
                clock,
                commits: BTreeMap::new(),
                view_timeouts: 0,
            },
            trace,
        }
    }

    pub fn state(&self) -> &GlobalState<CS> {
        &self.state
    }

    pub fn crash(&mut self, node_id: NodeID) {
        self.state.nodes.remove(&node_id);
        self.state.dormant.remove(&node_id);
        self.state.timers.remove(&node_id);
    }
}

impl<CS: ManualClock> Simulator<CS>
where
    CS::Clock: Debug,
{
    pub fn tick<DP: DeliveryPolicy<CS>>(
        &mut self,
        adversary: &mut Adversary<CS, DP>,
    ) -> Result<bool> {
        let Some((node_id, now, effects)) = self.advance_next_time()? else {
            return Ok(false);
        };
        self.execute_effects(now, node_id, effects, &mut adversary.delivery)?;
        Ok(true)
    }

    fn next_event(&mut self) -> Option<Event> {
        let at = self.state.next_time()?;
        self.state.timeline.advance_to(at);
        let due: Vec<NodeID> = self
            .state
            .timers
            .iter()
            .filter(|(_, timers)| {
                timers
                    .next_deadline()
                    .is_some_and(|deadline| deadline <= at)
            })
            .map(|(id, _)| *id)
            .collect();
        let pending = self.state.timeline.pending(at);
        let pick = self.state.timeline.choose(pending + due.len());
        match pick.checked_sub(pending) {
            None => self.state.timeline.take(at, pick),
            Some(index) => {
                let node_id = due[index];
                let timer = self
                    .state
                    .timers
                    .get_mut(&node_id)
                    .and_then(|timers| timers.pop_due(at))
                    .expect("a timer is due at the earliest deadline");
                Some(Event::Timer { node_id, timer })
            }
        }
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

        let traced = self.trace.is_some();
        let (node_id, inputs, label) = match event {
            Event::Start { node_id } => {
                if let Some(replica) = self.state.dormant.remove(&node_id) {
                    self.state.nodes.insert(node_id, replica);
                    self.state.timers.insert(node_id, TimerQueue::new());
                }
                (node_id, Vec::new(), traced.then(|| "start".to_owned()))
            }
            Event::Deliver { from, to, bytes } => {
                let msg = Message::<TestScheme, TestCommand>::decode_all(&mut &bytes[..])
                    .map_err(|err| anyhow!("undecodable message from {from:?}: {err:?}"))?;
                let label = traced.then(|| format!("{} from {}", describe(&msg), node(from)));
                (to, vec![msg.into_event(from)], label)
            }
            Event::Synced { node_id, blocks } => {
                let label = traced.then(|| format!("synced {} blocks", blocks.len()));
                let mut inputs = Vec::with_capacity(blocks.len());
                for bytes in blocks {
                    let block = Block::<TestScheme, TestCommand>::decode_all(&mut &bytes[..])
                        .map_err(|err| anyhow!("undecodable block: {err:?}"))?;
                    inputs.push(DriverInput::BlockSynced(block));
                }
                (node_id, inputs, label)
            }
            Event::Timer { node_id, timer } => {
                let label = traced.then(|| format!("**{timer:?}**"));
                let input = match timer {
                    TimerEvent::ViewTimeout(view) => {
                        self.state.view_timeouts += 1;
                        DriverInput::ViewTimerFired(view)
                    }
                    TimerEvent::LockTimeout(view) => DriverInput::LockTimerFired(view),
                };
                (node_id, vec![input], label)
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
        if let (Some(trace), Some(label)) = (&mut self.trace, label) {
            trace.step(now, node_id, &label, &effects)?;
        }
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
                Recipient::All => {
                    let mut all: Vec<NodeID> = self
                        .state
                        .nodes
                        .keys()
                        .chain(self.state.dormant.keys())
                        .copied()
                        .collect();
                    all.sort();
                    all
                }
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
