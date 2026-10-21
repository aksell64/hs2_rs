use crate::spec::Spec;
use anyhow::{Context, anyhow, bail};
use codec::{DecodeAll, Encode};
use hotstuff_consensus::mocks::genesis::genesis_config;
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::mempool::TestMempool;
use hotstuff_consensus::mocks::scheme::{TestCommand, TestDigest, TestScheme, TestSignature};
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::network::message::{Envelope, Message, Recipient};
use hotstuff_consensus::protocol::driver::DriverInput;
use hotstuff_consensus::protocol::leaders::RoundRobin;
use hotstuff_consensus::sm::NodeState;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::timer::service::TimerCommand;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::clock::ClockSource;
use hotstuff_consensus::types::crypto::Notary;
use hotstuff_consensus::types::genesis::{GenesisConfig, genesis_parent};
use hotstuff_crypto::sign::{Certificate, PartialSignature, Scheme};
use hotstuff_types::node::NodeID;
use quint_connect::*;
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

pub type S = TestScheme;
pub type C = TestCommand;
pub type Replica = NodeState<
    S,
    C,
    KVStorage<MemoryKV<Vec<u8>, Vec<u8>>>,
    TestMempool,
    FixedPolicy<u64>,
    Logical,
    RoundRobin,
>;

pub struct Logical;

impl ClockSource for Logical {
    type Clock = u64;

    fn now(&self) -> u64 {
        0
    }
}

#[derive(Default)]
pub struct Timers {
    pub view: BTreeSet<View>,
    pub lock: Option<View>,
}

impl Timers {
    fn apply(&mut self, cmd: TimerCommand<u64>) {
        match cmd {
            TimerCommand::SetEpochTimers(epoch) => {
                self.view.extend(epoch.timeouts.iter().map(|t| t.view))
            }
            TimerCommand::SetLockTimer(timeout) => self.lock = Some(timeout.view),
            TimerCommand::ClearLockTimer(view) => {
                if self.lock == Some(view) {
                    self.lock = None;
                }
            }
            TimerCommand::ClearViewTimersBelow(view) => self.view.retain(|w| *w >= view),
        }
    }
}

pub struct World {
    n: u32,
    faulty: BTreeSet<NodeID>,
    pub nodes: BTreeMap<NodeID, Replica>,
    pub timers: BTreeMap<NodeID, Timers>,
    pub inbox: BTreeMap<NodeID, VecDeque<(NodeID, Vec<u8>)>>,
    hashes: HashMap<TestDigest, i64>,
}

impl Driver for World {
    type State = Spec;

    fn step(&mut self, step: &Step) -> Result {
        switch!(step {
            init => self.init()?,
            apply_step(id: u32) => self.apply_step(NodeID::from(id))?,
            fire_view_timer(id: u32) => self.fire_view_timer(NodeID::from(id))?,
            fire_lock_timer(id: u32) => self.fire_lock_timer(NodeID::from(id))?,
            sync_step => bail!("blocksync is not wired yet"),
        })
    }
}

impl World {
    pub fn new(n: u32, faulty: impl IntoIterator<Item = u32>) -> Self {
        Self {
            n,
            faulty: faulty.into_iter().map(NodeID::from).collect(),
            nodes: BTreeMap::new(),
            timers: BTreeMap::new(),
            inbox: BTreeMap::new(),
            hashes: HashMap::new(),
        }
    }

    fn init(&mut self) -> Result {
        self.nodes.clear();
        self.timers.clear();
        self.inbox.clear();
        self.hashes.clear();

        let notary = Notary::new(Arc::new(TestScheme {
            n: self.n,
            id: None,
        }));
        self.learn(genesis_parent(&notary), -1)?;
        self.learn(genesis(self.n)?.qc.block(), 0)?;

        let running: Vec<NodeID> = (0..self.n)
            .map(NodeID::from)
            .filter(|id| !self.faulty.contains(id))
            .collect();
        for id in running {
            let replica = NodeState::new(
                Notary::new(Arc::new(TestScheme {
                    n: self.n,
                    id: Some(id),
                })),
                KVStorage::new(MemoryKV::new()),
                TestMempool::new(1),
                id,
                Logical,
                FixedPolicy {
                    view_timeout: 1,
                    lock_timeout: 1,
                },
                RoundRobin::new(self.n),
                genesis(self.n)?,
            )?;
            self.nodes.insert(id, replica);
            self.timers.insert(id, Timers::default());
            self.inbox.insert(id, VecDeque::new());
        }
        Ok(())
    }

    fn apply_step(&mut self, id: NodeID) -> Result {
        let node = self.nodes.get_mut(&id).context("node is not running")?;
        if let Some((sender, bytes)) = self.inbox.get_mut(&id).and_then(VecDeque::pop_front) {
            node.add_event_to_queue(decode(&bytes)?.into_event(sender));
        }
        let Some(effects) = node.apply()? else {
            return Ok(());
        };
        let timers = self.timers.get_mut(&id).context("node is not running")?;
        effects.timers.into_iter().for_each(|cmd| timers.apply(cmd));
        for envelope in effects.net {
            self.send(id, envelope)?;
        }
        Ok(())
    }

    fn fire_view_timer(&mut self, id: NodeID) -> Result {
        let view = self
            .timers
            .get_mut(&id)
            .and_then(|t| t.view.pop_first())
            .context("no view timer armed")?;
        self.node(id)?
            .add_event_to_queue(DriverInput::ViewTimerFired(view));
        Ok(())
    }

    fn fire_lock_timer(&mut self, id: NodeID) -> Result {
        let view = self
            .timers
            .get_mut(&id)
            .and_then(|t| t.lock.take())
            .context("no lock timer armed")?;
        self.node(id)?
            .add_event_to_queue(DriverInput::LockTimerFired(view));
        Ok(())
    }

    fn send(&mut self, sender: NodeID, envelope: Envelope<S, C>) -> Result {
        if let Message::ProposalMsg(proposal) = &envelope.msg {
            self.learn(
                proposal.block.hash,
                block_hash(self.n, proposal.view, sender),
            )?;
        }
        let receivers: Vec<NodeID> = match envelope.to {
            Recipient::All => self.inbox.keys().copied().collect(),
            Recipient::Node(id) => vec![id],
        };
        let bytes = envelope.msg.encode();
        for id in receivers {
            self.inbox
                .entry(id)
                .or_default()
                .push_back((sender, bytes.clone()));
        }
        Ok(())
    }

    fn learn(&mut self, digest: TestDigest, hash: i64) -> Result {
        match self.hashes.insert(digest, hash) {
            Some(known) if known != hash => bail!("{digest:?} is both {known} and {hash}"),
            _ => Ok(()),
        }
    }

    pub fn spec_hash(&self, digest: TestDigest) -> Result<i64> {
        self.hashes
            .get(&digest)
            .copied()
            .with_context(|| format!("unknown block {digest:?}"))
    }

    pub fn known_hashes(&self) -> impl Iterator<Item = (&TestDigest, &i64)> {
        self.hashes.iter()
    }

    fn node(&mut self, id: NodeID) -> Result<&mut Replica> {
        self.nodes.get_mut(&id).context("node is not running")
    }
}

fn genesis(n: u32) -> Result<GenesisConfig<S, C>> {
    let mut config = genesis_config(n, "mbt");
    let digest = config.qc.raw.data_hash();
    let signers = (0..n).map(|id| PartialSignature::new(NodeID::from(id), TestSignature));
    let aggregate = TestScheme { n, id: None }
        .aggregate(digest, signers)
        .map_err(|err| anyhow!("{err:?}"))?;
    config.qc.raw = Certificate::new(digest, aggregate);
    Ok(config)
}

fn block_hash(n: u32, view: View, proposer: NodeID) -> i64 {
    (view as i64 * i64::from(n) + i64::from(u32::from(proposer))) * 2
}

pub fn decode(bytes: &[u8]) -> Result<Message<S, C>> {
    Message::decode_all(&mut &bytes[..]).map_err(|err| anyhow!("{err:?}"))
}
