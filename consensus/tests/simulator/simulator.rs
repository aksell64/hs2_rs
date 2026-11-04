use crate::simulator::adversary::{
    Adversary, DeliveryContext, DeliveryPolicy, Fate, Packet, Payload,
};
use crate::simulator::byzantine::{ByzantineContext, ByzantineProxy};
use crate::simulator::clock::Timeline;
use crate::simulator::rng::SharedRng;
use crate::simulator::trace::{Snapshot, Trace, describe, node};
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
use std::collections::{BTreeMap, BTreeSet};
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

pub type NodeEffects<CS> = Effects<TestScheme, TestCommand, <CS as ClockSource>::Clock>;

pub fn no_effects<CL: Clock>() -> Effects<TestScheme, TestCommand, CL> {
    Effects {
        net: Vec::new(),
        timers: Vec::new(),
        sync: Vec::new(),
        committed: Vec::new(),
    }
}

fn merge<CL: Clock>(
    into: &mut Effects<TestScheme, TestCommand, CL>,
    other: Effects<TestScheme, TestCommand, CL>,
) {
    into.net.extend(other.net);
    into.timers.extend(other.timers);
    into.sync.extend(other.sync);
    into.committed.extend(other.committed);
}

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
    unstarted: BTreeMap<NodeID, Replica<CS>>,
    timeline: Timeline<CS::Clock>,
    timers: BTreeMap<NodeID, TimerQueue<CS::Clock>>,
    stable_net_delay: <<CS as ClockSource>::Clock as Clock>::Duration,
    clock: CS,
    commits: BTreeMap<NodeID, Vec<Commit<CS::Clock>>>,
    view_timeouts: u64,
    byzantine: BTreeSet<NodeID>,
}

impl<CS: ClockSource> GlobalState<CS> {
    pub fn now(&self) -> CS::Clock {
        self.timeline.now()
    }

    pub fn running(&self) -> impl Iterator<Item = NodeID> + '_ {
        self.nodes.keys().copied()
    }

    pub fn honest_nodes(&self) -> impl Iterator<Item = NodeID> + '_ {
        self.running().filter(|id| !self.is_byzantine(*id))
    }

    pub fn is_byzantine(&self, node_id: NodeID) -> bool {
        self.byzantine.contains(&node_id)
    }

    pub fn replica(&self, node_id: NodeID) -> Option<&Replica<CS>> {
        self.nodes.get(&node_id)
    }

    pub fn honest_commit_logs(&self) -> impl Iterator<Item = (NodeID, &[Commit<CS::Clock>])> {
        self.commits
            .iter()
            .filter(|(id, _)| !self.is_byzantine(**id))
            .map(|(id, log)| (*id, log.as_slice()))
    }

    pub fn honest_views(&self) -> impl Iterator<Item = (NodeID, View)> {
        self.nodes
            .iter()
            .filter(|(id, _)| !self.is_byzantine(**id))
            .map(|(id, node)| {
                let view = ReadStorage::<TestScheme, TestCommand>::get_cur_view(node.storage());
                (*id, view.unwrap())
            })
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
    rng: SharedRng,
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
        rng: SharedRng,
        byzantine: BTreeSet<NodeID>,
        trace: Option<Trace>,
    ) -> Self {
        let mut timeline = Timeline::new(clock.now());
        for node_id in nodes.keys().copied() {
            let at = starts.get(&node_id).copied().unwrap_or(clock.now());
            timeline.add_event(Event::Start { node_id }, at);
        }
        Self {
            state: GlobalState {
                nodes: BTreeMap::new(),
                unstarted: nodes,
                timeline,
                timers: BTreeMap::new(),
                stable_net_delay,
                clock,
                commits: BTreeMap::new(),
                view_timeouts: 0,
                byzantine,
            },
            trace,
            rng,
        }
    }

    pub fn state(&self) -> &GlobalState<CS> {
        &self.state
    }

    pub fn crash(&mut self, node_id: NodeID) {
        self.state.nodes.remove(&node_id);
        self.state.unstarted.remove(&node_id);
        self.state.timers.remove(&node_id);
    }

    pub fn note(&mut self, note: String) {
        if let Some(trace) = &mut self.trace {
            trace.note(note);
        }
    }

    pub fn mark(&mut self, at: CS::Clock, label: &str) {
        if let Some(trace) = &mut self.trace {
            trace.mark(at, label);
        }
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
        let byzantine = &mut adversary.byzantine;
        let Some((node_id, now, effects)) = self.advance_next_time(byzantine)? else {
            return Ok(false);
        };
        self.execute_effects(now, node_id, effects, &mut adversary.delivery)?;
        Ok(true)
    }

    fn next_event(&mut self) -> Option<Event> {
        let at = self.state.next_time()?;
        self.state.timeline.advance_to(at);
        let timer_nodes: Vec<NodeID> = self
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

        let pending_timers = timer_nodes.len();
        let pending_events = self.state.timeline.pending(at);
        let candidates = pending_events + pending_timers;
        let pick = if candidates > 1 {
            self.rng.range(0, candidates as u64 - 1) as usize
        } else {
            0
        };

        if pick < pending_events {
            self.state.timeline.take(at, pick)
        } else {
            let node_id = timer_nodes[pick - pending_events];
            let timer = self
                .state
                .timers
                .get_mut(&node_id)
                .and_then(|timers| timers.pop_due(at))
                .expect("a timer is due at the earliest deadline");
            Some(Event::Timer { node_id, timer })
        }
    }

    fn advance_next_time(
        &mut self,
        byzantine: &mut BTreeMap<NodeID, ByzantineProxy<CS>>,
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
                if let Some(replica) = self.state.unstarted.remove(&node_id) {
                    self.state.nodes.insert(node_id, replica);
                    self.state.timers.insert(node_id, TimerQueue::new());
                }
                (node_id, Vec::new(), traced.then(|| "start".to_owned()))
            }
            Event::Deliver { from, to, bytes } => {
                let msg = Message::<TestScheme, TestCommand>::decode_all(&mut &bytes[..])
                    .map_err(|err| anyhow!("undecodable message from {from:?}: {err:?}"))?;
                let label =
                    traced.then(|| format!("received {} from {}", describe(&msg), node(from)));
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

        let proxy = byzantine
            .get_mut(&node_id)
            .and_then(|proxy| (proxy.activate_when)(&self.state).then_some(proxy));
        let label = match &proxy {
            Some(_) => label.map(|label| format!("{label} [byzantine]")),
            None => label,
        };
        let label = if self.state.nodes.contains_key(&node_id) {
            label
        } else if self.state.unstarted.contains_key(&node_id) {
            label.map(|label| format!("{label} [not started]"))
        } else {
            label.map(|label| format!("{label} [crashed]"))
        };
        if let Some(replica) = self.state.nodes.get_mut(&node_id) {
            for input in inputs {
                replica.add_event_to_queue(input);
            }
        }
        let effects = match proxy {
            Some(proxy) => self.apply_byzantine(node_id, proxy)?,
            None => match self.state.nodes.get_mut(&node_id) {
                Some(replica) => replica.apply()?,
                None => no_effects(),
            },
        };
        if let Some(label) = label {
            let snapshot = self.snapshot(node_id);
            if let Some(trace) = &mut self.trace {
                trace.step(now, node_id, &label, &effects, snapshot);
            }
        }
        Ok(Some((node_id, now, effects)))
    }

    fn snapshot(&self, node_id: NodeID) -> Option<Snapshot> {
        const READ: &str = "in-memory storage read failed";
        let storage: &dyn ReadStorage<TestScheme, TestCommand> =
            self.state.nodes.get(&node_id)?.storage();
        let high_qc = storage.get_high_qc().expect(READ);
        let view = storage.get_cur_view().expect(READ);
        Some(Snapshot {
            view,
            high_qc: high_qc.view(),
            locked: storage.get_locked_qc().expect(READ).view(),
            exec: storage.get_exec_height().expect(READ),
            proposed: storage.get_proposed_view().expect(READ) == view,
            missing_high_qc_block: storage
                .get_block_by_hash(&high_qc.block())
                .expect(READ)
                .is_none(),
        })
    }

    fn apply_byzantine(
        &mut self,
        node_id: NodeID,
        proxy: &mut ByzantineProxy<CS>,
    ) -> Result<NodeEffects<CS>> {
        let Some(mut replica) = self.state.nodes.remove(&node_id) else {
            return Ok(no_effects());
        };
        let effects = Self::run_behavior(node_id, &mut replica, &self.state, proxy);
        self.state.nodes.insert(node_id, replica);
        effects
    }

    fn run_behavior(
        node_id: NodeID,
        replica: &mut Replica<CS>,
        state: &GlobalState<CS>,
        proxy: &mut ByzantineProxy<CS>,
    ) -> Result<NodeEffects<CS>> {
        let mut effects = no_effects();
        while let Some(input) = replica.pop_input() {
            let mut context = ByzantineContext::new(node_id, replica, state);
            merge(&mut effects, (proxy.behavior)(&mut context, input)?);
        }
        Ok(effects)
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
                        .chain(self.state.unstarted.keys())
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
