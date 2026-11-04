pub mod simulator;

use hotstuff_consensus::mocks::blocksync::NoBlockSync;
use hotstuff_consensus::mocks::genesis::genesis_config;
use hotstuff_consensus::mocks::kv::MemoryKV;
use hotstuff_consensus::mocks::mempool::TestMempool;
use hotstuff_consensus::mocks::network::MemoryNetwork;
use hotstuff_consensus::mocks::output::{CommitLog, TestOutput};
use hotstuff_consensus::mocks::scheme::{TestCommand, TestScheme};
use hotstuff_consensus::mocks::timeouts::FixedPolicy;
use hotstuff_consensus::node::Node;
use hotstuff_consensus::protocol::leaders::RoundRobin;
use hotstuff_consensus::sm::NodeError;
use hotstuff_consensus::storage::kv_storage::KVStorage;
use hotstuff_consensus::timer::service::{TokioSource, TokioTimerService};
use hotstuff_types::node::NodeID;
use std::collections::BTreeSet;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout};

const COMMAND_SPACE: u64 = 1_000_000;

struct Cluster {
    logs: Vec<CommitLog>,
    handles: Vec<JoinHandle<Result<(), NodeError>>>,
}

fn spawn_cluster(n: u32) -> Cluster {
    let mut logs = Vec::new();
    let mut handles = Vec::new();
    for network in MemoryNetwork::connect((0..n).map(NodeID::from)) {
        let id = network.id();
        let log = CommitLog::default();
        let (node, _) = Node::new(
            TestScheme { n, id: Some(id) },
            network,
            NoBlockSync,
            TokioTimerService::new(),
            KVStorage::new(MemoryKV::<Vec<u8>, Vec<u8>>::new()),
            TestMempool::new(u64::from(u32::from(id)) * COMMAND_SPACE + 1),
            FixedPolicy::<Instant> {
                view_timeout: Duration::from_millis(500),
                lock_timeout: Duration::from_millis(100),
            },
            TokioSource,
            RoundRobin::new(n),
            TestOutput::new(log.clone()),
            genesis_config(n, "e2e"),
            id,
        )
        .expect("node starts");
        logs.push(log);
        handles.push(tokio::spawn(node.run()));
    }
    Cluster { logs, handles }
}

fn snapshot(logs: &[CommitLog]) -> Vec<Vec<TestCommand>> {
    logs.iter()
        .map(|log| {
            log.lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        })
        .collect()
}

async fn wait_for_commits(logs: &[CommitLog], commits: usize) {
    while snapshot(logs).iter().any(|log| log.len() < commits) {
        sleep(Duration::from_millis(10)).await;
    }
}

async fn run_until_commits(
    cluster: Cluster,
    commits: usize,
    limit: Duration,
) -> Vec<Vec<TestCommand>> {
    if timeout(limit, wait_for_commits(&cluster.logs, commits))
        .await
        .is_err()
    {
        for handle in cluster.handles {
            if handle.is_finished() {
                panic!("a node stopped: {:?}", handle.await);
            }
        }
        let lengths: Vec<usize> = snapshot(&cluster.logs).iter().map(Vec::len).collect();
        panic!("no {commits} commits on every node within {limit:?}, got {lengths:?}");
    }
    for handle in &cluster.handles {
        handle.abort();
    }
    snapshot(&cluster.logs)
}

fn assert_consistent(logs: &[Vec<TestCommand>]) {
    let shortest = logs.iter().map(Vec::len).min().unwrap_or(0);
    for log in logs {
        assert_eq!(log[..shortest], logs[0][..shortest]);
        let unique: BTreeSet<u64> = log.iter().map(|cmd| cmd.0).collect();
        assert_eq!(
            unique.len(),
            log.len(),
            "a command is committed twice: {log:?}"
        );
    }
}

#[tokio::test]
async fn four_correct_nodes_commit_the_same_chain() {
    let cluster = spawn_cluster(4);
    let logs = run_until_commits(cluster, 5, Duration::from_secs(10)).await;
    assert_consistent(&logs);
}
