use hotstuff_consensus::mocks::scheme::{TestCommand, TestDigest, TestScheme};
use hotstuff_consensus::network::message::Message;
use hotstuff_consensus::protocol::leaders::{LeaderPolicy, RoundRobin};
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::block::Block;
use hotstuff_types::node::NodeID;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;

pub struct NodeSnapshot {
    pub view: View,
    pub leader: NodeID,
    pub proposed: bool,
    pub high_qc: (View, TestDigest),
    pub lock: (View, TestDigest),
    pub voted_view: View,
    pub exec_height: u64,
    pub exec_hash: TestDigest,
    pub committed: Vec<TestDigest>,
    pub stored: Vec<TestDigest>,
    pub view_timers: Vec<String>,
    pub lock_timer: Option<String>,
}

pub struct ViewerTrace {
    title: String,
    setup: String,
    n: u32,
    byzantine: BTreeSet<u32>,
    crashed: BTreeSet<u32>,
    block_ids: BTreeMap<[u8; 4], u64>,
    digests: Vec<TestDigest>,
    blocks: BTreeMap<u64, BlockRecord>,
    certified: BTreeSet<(View, u64)>,
    qcs: Vec<QcRecord>,
    messages: Vec<MessageRecord>,
    steps: Vec<Step>,
    marks: Vec<MarkRecord>,
    sync: SyncCost,
}

#[derive(Clone, Copy, Default, Serialize)]
struct SyncCost {
    responses: u64,
    blocks: u64,
    redundant: u64,
}

#[derive(Serialize)]
struct Data<'a> {
    meta: Meta<'a>,
    blocks: Vec<&'a BlockRecord>,
    qcs: &'a [QcRecord],
    messages: &'a [MessageRecord],
    steps: &'a [Step],
    marks: &'a [MarkRecord],
}

#[derive(Serialize)]
struct MarkRecord {
    step: usize,
    time: String,
    label: String,
}

#[derive(Serialize)]
struct Meta<'a> {
    model: &'a str,
    source: String,
    steps: usize,
    running: Vec<u32>,
    hosts: Vec<u32>,
    faulty: BTreeSet<u32>,
    faulty_label: &'static str,
    committee_size: u32,
    epoch_length: View,
    sync: SyncCost,
    warnings: Vec<String>,
}

#[derive(Serialize)]
struct BlockRecord {
    hash: u64,
    parent: Option<u64>,
    height: u64,
    justify: Qc,
    minted: usize,
    view: Option<View>,
    proposer: Option<u32>,
}

#[derive(Clone, Copy, Serialize)]
struct Qc {
    block: u64,
    view: View,
    signers: &'static str,
}

#[derive(Serialize)]
struct QcRecord {
    #[serde(flatten)]
    qc: Qc,
    step: usize,
}

#[derive(Serialize)]
struct MessageRecord {
    id: u64,
    from: u32,
    to: u32,
    kind: &'static str,
    text: String,
    block: Option<u64>,
    channel: &'static str,
    send: usize,
    recv: Option<usize>,
    dropped: bool,
}

#[derive(Serialize)]
struct Step {
    action: &'static str,
    actor: Option<u32>,
    label: String,
    time: String,
    tags: BTreeSet<&'static str>,
    nodes: BTreeMap<u32, NodeRecord>,
}

#[derive(Serialize)]
struct NodeRecord {
    view: View,
    leader: u32,
    proposed: bool,
    high_qc: Qc,
    lock: Qc,
    voted_view: View,
    exec_height: u64,
    exec_hash: u64,
    committed: Vec<u64>,
    stored: Vec<u64>,
    view_timers: Vec<String>,
    lock_timer: Option<String>,
}

impl ViewerTrace {
    pub fn new(
        title: &str,
        setup: &str,
        n: u32,
        byzantine: &BTreeSet<NodeID>,
        crashed: &BTreeSet<NodeID>,
    ) -> Self {
        Self {
            title: title.to_owned(),
            setup: setup.to_owned(),
            n,
            byzantine: byzantine.iter().copied().map(u32::from).collect(),
            crashed: crashed.iter().copied().map(u32::from).collect(),
            block_ids: BTreeMap::new(),
            digests: Vec::new(),
            blocks: BTreeMap::new(),
            certified: BTreeSet::new(),
            qcs: Vec::new(),
            messages: Vec::new(),
            steps: Vec::new(),
            marks: Vec::new(),
            sync: SyncCost::default(),
        }
    }

    pub fn known_blocks(&self) -> &[TestDigest] {
        &self.digests
    }

    pub fn begin<CL: Debug>(
        &mut self,
        now: CL,
        actor: Option<NodeID>,
        action: &'static str,
        label: String,
    ) {
        self.steps.push(Step {
            action,
            actor: actor.map(u32::from),
            label,
            time: format!("{now:?}"),
            tags: BTreeSet::new(),
            nodes: BTreeMap::new(),
        });
        match action {
            "view_timeout" | "lock_timeout" => self.tag("timer"),
            "synced" => self.tag("sync"),
            _ => {}
        }
    }

    pub fn tag(&mut self, tag: &'static str) {
        if let Some(step) = self.steps.last_mut() {
            step.tags.insert(tag);
        }
    }

    pub fn mark<CL: Debug>(&mut self, at: CL, label: &str) {
        self.marks.push(MarkRecord {
            step: self.current_step(),
            time: format!("{at:?}"),
            label: label.to_owned(),
        });
        self.tag("mark");
    }

    pub fn synced(&mut self, blocks: usize, redundant: usize) {
        self.sync.responses += 1;
        self.sync.blocks += blocks as u64;
        self.sync.redundant += redundant as u64;
        if let Some(step) = self.steps.last_mut() {
            step.label
                .push_str(&format!(" ({redundant} already stored)"));
        }
    }

    pub fn block(
        &mut self,
        block: &Block<TestScheme, TestCommand>,
        view: Option<View>,
        proposer: Option<NodeID>,
    ) {
        let hash = self.block_id(block.hash);
        if self.blocks.contains_key(&hash) {
            return;
        }
        let parent = (block.height > 0).then(|| self.block_id(block.parent));
        let justify = self.qc(block.justify.view(), block.justify.block());
        let record = BlockRecord {
            hash,
            parent,
            height: block.height,
            justify,
            minted: self.current_step(),
            view,
            proposer: proposer.map(u32::from),
        };
        self.blocks.insert(hash, record);
    }

    pub fn sent(
        &mut self,
        id: u64,
        from: NodeID,
        to: NodeID,
        msg: &Message<TestScheme, TestCommand>,
        dropped: bool,
    ) {
        if let Message::ProposalMsg(proposal) = msg {
            self.block(&proposal.block, Some(proposal.view), Some(from));
        }
        if dropped {
            self.tag("drop");
        }
        let record = MessageRecord {
            id,
            from: from.into(),
            to: to.into(),
            kind: kind(msg),
            text: self.text(msg),
            block: self.message_block(msg),
            channel: "net",
            send: self.current_step(),
            recv: None,
            dropped,
        };
        self.messages.push(record);
    }

    pub fn received(&mut self, id: u64) {
        let step = self.current_step();
        self.messages[id as usize].recv = Some(step);
    }

    pub fn finish(&mut self, nodes: BTreeMap<NodeID, NodeSnapshot>) {
        let nodes: BTreeMap<u32, NodeRecord> = nodes
            .into_iter()
            .map(|(id, node)| (id.into(), self.node_record(node)))
            .collect();
        let changed = |get: fn(&NodeRecord) -> u64| match self.steps.as_slice() {
            [.., prev, _] => nodes
                .iter()
                .any(|(id, node)| prev.nodes.get(id).is_some_and(|old| get(old) != get(node))),
            _ => false,
        };
        let (view_changed, exec_changed) = (changed(|n| n.view), changed(|n| n.exec_height));
        if view_changed {
            self.tag("view");
        }
        if exec_changed {
            self.tag("commit");
        }
        if let Some(step) = self.steps.last_mut() {
            step.nodes = nodes;
        }
    }

    pub fn write(&self, path: &Path, notes: &[String]) -> io::Result<()> {
        let mut source = self.setup.clone();
        for note in notes {
            source.push_str(" · ");
            source.push_str(note);
        }
        let faulty: BTreeSet<u32> = self.byzantine.union(&self.crashed).copied().collect();
        let faulty_label = match (self.byzantine.is_empty(), self.crashed.is_empty()) {
            (false, true) => "byzantine",
            (true, false) => "crashed",
            _ => "faulty",
        };
        let data = Data {
            meta: Meta {
                model: &self.title,
                source,
                steps: self.current_step(),
                running: (0..self.n)
                    .filter(|id| !self.crashed.contains(id))
                    .collect(),
                hosts: (0..self.n).collect(),
                faulty,
                faulty_label,
                committee_size: self.n,
                epoch_length: RoundRobin::new(self.n).epoch_length(),
                sync: self.sync,
                warnings: Vec::new(),
            },
            blocks: self.blocks.values().collect(),
            qcs: &self.qcs,
            messages: &self.messages,
            steps: &self.steps,
            marks: &self.marks,
        };
        let mut out = BufWriter::new(File::create(path)?);
        serde_json::to_writer(&mut out, &data)?;
        out.flush()
    }

    fn current_step(&self) -> usize {
        self.steps.len().saturating_sub(1)
    }

    fn block_id(&mut self, digest: TestDigest) -> u64 {
        let next = self.digests.len() as u64;
        let id = *self.block_ids.entry(digest.0).or_insert(next);
        if id == next {
            self.digests.push(digest);
        }
        id
    }

    fn qc(&mut self, view: View, block: TestDigest) -> Qc {
        let qc = Qc {
            block: self.block_id(block),
            view,
            signers: "",
        };
        if self.certified.insert((view, qc.block)) {
            let step = self.current_step();
            self.qcs.push(QcRecord { qc, step });
            if step > 0 {
                self.tag("qc");
            }
        }
        qc
    }

    fn node_record(&mut self, node: NodeSnapshot) -> NodeRecord {
        NodeRecord {
            view: node.view,
            leader: node.leader.into(),
            proposed: node.proposed,
            high_qc: self.qc(node.high_qc.0, node.high_qc.1),
            lock: self.qc(node.lock.0, node.lock.1),
            voted_view: node.voted_view,
            exec_height: node.exec_height,
            exec_hash: self.block_id(node.exec_hash),
            committed: node
                .committed
                .into_iter()
                .map(|d| self.block_id(d))
                .collect(),
            stored: node.stored.into_iter().map(|d| self.block_id(d)).collect(),
            view_timers: node.view_timers,
            lock_timer: node.lock_timer,
        }
    }

    fn message_block(&mut self, msg: &Message<TestScheme, TestCommand>) -> Option<u64> {
        match msg {
            Message::ProposalMsg(proposal) => Some(self.block_id(proposal.block.hash)),
            Message::VoteMsg(vote) => Some(self.block_id(vote.block_hash())),
            Message::StatusMsg { qc, .. } => Some(self.block_id(qc.block())),
            Message::WishMsg(_) | Message::TCMsg(_) => None,
        }
    }

    fn text(&mut self, msg: &Message<TestScheme, TestCommand>) -> String {
        match msg {
            Message::ProposalMsg(proposal) => {
                let block = &proposal.block;
                format!(
                    "Proposal(v{}, b{} h{}, QC(b{}@v{}))",
                    proposal.view,
                    self.block_id(block.hash),
                    block.height,
                    self.block_id(block.justify.block()),
                    block.justify.view()
                )
            }
            Message::VoteMsg(vote) => {
                format!(
                    "Vote(v{}, b{})",
                    vote.view(),
                    self.block_id(vote.block_hash())
                )
            }
            Message::WishMsg(wish) => format!("Wish(v{})", wish.view()),
            Message::StatusMsg { qc, view } => {
                format!(
                    "Status(v{view}, QC(b{}@v{}))",
                    self.block_id(qc.block()),
                    qc.view()
                )
            }
            Message::TCMsg(tc) => format!("TC(v{})", tc.view()),
        }
    }
}

fn kind(msg: &Message<TestScheme, TestCommand>) -> &'static str {
    match msg {
        Message::ProposalMsg(_) => "Proposal",
        Message::VoteMsg(_) => "Vote",
        Message::WishMsg(_) => "Wish",
        Message::StatusMsg { .. } => "Status",
        Message::TCMsg(_) => "TC",
    }
}

pub fn record_violation(path: &Path, violation: &str) -> io::Result<()> {
    let mut data: Value = serde_json::from_str(&fs::read_to_string(path)?)?;
    let last = data["steps"]
        .as_array()
        .map_or(0, |steps| steps.len().saturating_sub(1));
    let time = data["steps"][last]["time"].clone();
    if let Some(marks) = data["marks"].as_array_mut() {
        marks.push(json!({ "step": last, "time": time, "label": violation, "violation": true }));
    }
    if let Some(tags) = data["steps"][last]["tags"].as_array_mut()
        && !tags.contains(&json!("mark"))
    {
        tags.push(json!("mark"));
    }
    let meta = &mut data["meta"];
    let source = meta["source"].as_str().unwrap_or_default();
    meta["source"] = format!("{source} · violation: {violation}").into();
    meta["warnings"] = vec![violation].into();
    fs::write(path, data.to_string())
}
