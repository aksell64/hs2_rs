use codec::{Decode, Encode};
use hotstuff_consensus::mocks::blocksync::NoBlockSync;
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::network::MemoryNetwork;
use hotstuff_consensus::mocks::scheme::{TestScheme, qc};
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::node::{Client, Node};
use hotstuff_consensus::protocol::leaders::RoundRobin;
use hotstuff_consensus::sm::NodeError;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::timer::service::{TokioSource, TokioTimerService};
use hotstuff_consensus::types::block::{Block, Command};
use hotstuff_consensus::types::crypto::Notary;
use hotstuff_consensus::types::genesis::{GenesisConfig, genesis_hash};
use hotstuff_consensus::types::mempool::Mempool;
use hotstuff_consensus::types::output::Output;
use hotstuff_types::node::NodeID;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::Instant;

const MAX_BATCH: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq, Encode, Decode)]
pub struct Message {
    pub sender: NodeID,
    pub payload: Vec<u8>,
}

#[derive(Clone, Debug, Default, Encode, Decode)]
pub struct Batch(pub Vec<Message>);

impl Command for Batch {}

pub struct BatchMempool {
    pending: VecDeque<Message>,
    max_batch: usize,
}

impl BatchMempool {
    pub fn new(max_batch: usize) -> Self {
        Self {
            pending: VecDeque::new(),
            max_batch,
        }
    }
}

impl Mempool<Batch> for BatchMempool {
    fn add_command(&mut self, cmd: Batch) {
        self.pending.extend(cmd.0);
    }

    fn next_command(&mut self) -> Batch {
        let size = self.pending.len().min(self.max_batch);
        Batch(self.pending.drain(..size).collect())
    }

    fn on_commands_commit<'a>(&mut self, _: impl IntoIterator<Item = &'a Batch>)
    where
        Batch: 'a,
    {
    }
}

pub struct Deliveries {
    tx: mpsc::UnboundedSender<Message>,
}

impl Output<TestScheme, Batch> for Deliveries {
    fn on_committed(&mut self, block: Block<TestScheme, Batch>) {
        for message in block.cmd.0 {
            let _ = self.tx.send(message);
        }
    }
}

pub struct Tob {
    id: NodeID,
    client: Client<Batch>,
    deliveries: mpsc::UnboundedReceiver<Message>,
    node: JoinHandle<Result<(), NodeError>>,
}

impl Tob {
    pub fn id(&self) -> NodeID {
        self.id
    }

    pub fn broadcast(&self, payload: impl Into<Vec<u8>>) {
        let message = Message {
            sender: self.id,
            payload: payload.into(),
        };
        self.client.propose(Batch(vec![message]));
    }

    pub async fn deliver(&mut self) -> Option<Message> {
        self.deliveries.recv().await
    }
}

impl Drop for Tob {
    fn drop(&mut self) {
        self.node.abort();
    }
}

pub fn genesis(n: u32, tag: &str) -> GenesisConfig<TestScheme, Batch> {
    let scheme = TestScheme { n, id: None };
    let notary = Notary::new(Arc::new(TestScheme { n, id: None }));
    let cmd = Batch::default();
    let hash = genesis_hash(&notary, &cmd, tag.as_bytes());
    GenesisConfig {
        qc: qc(&scheme, 0, hash),
        cmd,
        tag: tag.to_string(),
    }
}

pub fn spawn_cluster(n: u32) -> Result<Vec<Tob>, NodeError> {
    MemoryNetwork::connect((0..n).map(NodeID::from))
        .into_iter()
        .map(|network| {
            let id = network.id();
            let (tx, deliveries) = mpsc::unbounded_channel();
            let (node, client) = Node::new(
                TestScheme { n, id: Some(id) },
                network,
                NoBlockSync,
                TokioTimerService::new(),
                KVStorage::new(MemoryKV::<Vec<u8>, Vec<u8>>::new()),
                BatchMempool::new(MAX_BATCH),
                FixedPolicy::<Instant> {
                    view_timeout: Duration::from_millis(500),
                    lock_timeout: Duration::from_millis(100),
                },
                TokioSource,
                RoundRobin::new(n),
                Deliveries { tx },
                genesis(n, "tob"),
                id,
            )?;
            Ok(Tob {
                id,
                client,
                deliveries,
                node: tokio::spawn(node.run()),
            })
        })
        .collect()
}
