use crate::world::{C, Replica, S, World, decode};
use anyhow::bail;
use hotstuff_consensus::network::message::Message;
use hotstuff_consensus::storage::ReadStorage;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::block::Block;
use hotstuff_consensus::types::certificate::{BlockQC, QuorumCertificate};
use hotstuff_types::node::NodeID;
use itf::Value;
use itf::de::{self, As};
use itf::value::Record;
use quint_connect::{Result, State};
use serde::Deserialize;
use serde::de::IgnoredAny;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq, Deserialize)]
pub struct Spec {
    nodes: BTreeMap<u32, SpecNode>,
    net: SpecNet,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecNode {
    driver: SpecDriver,
    kv: SpecKv,
    view_timers: BTreeSet<View>,
    #[serde(with = "As::<de::Option::<_>>")]
    lock_timer: Option<View>,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecDriver {
    consensus: SpecConsensus,
    pacemaker: SpecPacemaker,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecConsensus {
    qc_lock: SpecQC,
    voted_view: View,
    exec_hash: i64,
    exec_height: u64,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecPacemaker {
    high_qc: SpecQC,
    cur_view: View,
    cur_leader: u32,
    proposed: bool,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecKv {
    block_by_hash: BTreeMap<i64, IgnoredAny>,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecQC {
    certificate: SpecCert,
    block: i64,
    view: View,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecCert {
    signature: SpecSignature,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "tag", content = "value")]
enum SpecSignature {
    QuorumCert(BTreeSet<u32>),
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecBlock {
    hash: i64,
    parent: i64,
    height: u64,
    justify: SpecQC,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecNet {
    inbox: BTreeMap<u32, Vec<SpecPacket>>,
}

#[derive(Debug, PartialEq, Deserialize)]
struct SpecPacket {
    sender: u32,
    msg: SpecMessage,
}

#[derive(Debug, PartialEq, Deserialize)]
#[serde(tag = "tag", content = "value")]
enum SpecMessage {
    ProposalMsg { block: SpecBlock, view: View },
    VoteMsg { view: View, block: i64 },
    WishMsg(View),
    StatusMsg { qc: SpecQC, view: View },
    TCMsg { certificate: SpecCert, view: View },
}

impl State<World> for Spec {
    fn from_spec(value: Value) -> Result<Self> {
        let Value::Record(record) = value else {
            bail!("spec state is not a record");
        };
        let mut short = Record::new();
        for (key, value) in record {
            let name = key.rsplit("::").next().unwrap_or(&key).to_string();
            short.insert(name, value);
        }
        Ok(Self::deserialize(Value::Record(short))?)
    }

    fn from_driver(world: &World) -> Result<Self> {
        let mut nodes = BTreeMap::new();
        for (id, node) in &world.nodes {
            nodes.insert(u32::from(*id), project_node(world, *id, node)?);
        }
        let mut inbox = BTreeMap::new();
        for (id, packets) in &world.inbox {
            let packets = packets
                .iter()
                .map(|(sender, bytes)| project_packet(world, *sender, bytes))
                .collect::<Result<_>>()?;
            inbox.insert(u32::from(*id), packets);
        }
        Ok(Spec { nodes, net: SpecNet { inbox } })
    }
}

fn read(node: &Replica) -> &impl ReadStorage<S, C> {
    node.storage()
}

fn project_node(world: &World, id: NodeID, node: &Replica) -> Result<SpecNode> {
    let db = read(node);
    let mut block_by_hash = BTreeMap::new();
    for (digest, hash) in world.known_hashes() {
        if db.get_block_by_hash(digest)?.is_some() {
            block_by_hash.insert(*hash, IgnoredAny);
        }
    }
    let timers = &world.timers[&id];
    Ok(SpecNode {
        driver: SpecDriver {
            consensus: SpecConsensus {
                qc_lock: project_qc(world, &db.get_locked_qc()?)?,
                voted_view: db.get_voted_view()?,
                exec_hash: world.spec_hash(db.get_exec_hash()?)?,
                exec_height: db.get_exec_height()?,
            },
            pacemaker: SpecPacemaker {
                high_qc: project_qc(world, &db.get_high_qc()?)?,
                cur_view: db.get_cur_view()?,
                cur_leader: u32::from(db.get_cur_leader()?),
                proposed: db.get_proposed()?,
            },
        },
        kv: SpecKv { block_by_hash },
        view_timers: timers.view.clone(),
        lock_timer: timers.lock,
    })
}

fn project_packet(world: &World, sender: NodeID, bytes: &[u8]) -> Result<SpecPacket> {
    let msg = match decode(bytes)? {
        Message::ProposalMsg(proposal) => SpecMessage::ProposalMsg {
            block: project_block(world, &proposal.block)?,
            view: proposal.view,
        },
        Message::VoteMsg(vote) => SpecMessage::VoteMsg {
            view: vote.view(),
            block: world.spec_hash(vote.block_hash())?,
        },
        Message::WishMsg(wish) => SpecMessage::WishMsg(wish.view()),
        Message::StatusMsg { qc, view } => SpecMessage::StatusMsg {
            qc: project_qc(world, &qc)?,
            view,
        },
        Message::TCMsg(tc) => SpecMessage::TCMsg {
            certificate: project_cert(&tc),
            view: tc.view(),
        },
    };
    Ok(SpecPacket { sender: u32::from(sender), msg })
}

fn project_block(world: &World, block: &Block<S, C>) -> Result<SpecBlock> {
    Ok(SpecBlock {
        hash: world.spec_hash(block.hash)?,
        parent: world.spec_hash(block.parent)?,
        height: block.height,
        justify: project_qc(world, &block.justify)?,
    })
}

fn project_qc(world: &World, qc: &BlockQC<S>) -> Result<SpecQC> {
    Ok(SpecQC {
        certificate: project_cert(qc),
        block: world.spec_hash(qc.block())?,
        view: qc.view(),
    })
}

fn project_cert<T>(qc: &QuorumCertificate<S, T>) -> SpecCert {
    let signers = qc.raw.aggregate().signers().iter().map(|id| u32::from(*id)).collect();
    SpecCert { signature: SpecSignature::QuorumCert(signers) }
}
