// Trace in the format of docs/spec/quint/tools/viewer.html (the same as tools/viz.py --json).
use hotstuff_consensus::mocks::scheme::{TestCommand, TestDigest, TestScheme};
use hotstuff_consensus::network::message::Message;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::block::Block;
use hotstuff_consensus::types::certificate::QuorumCertificate;
use hotstuff_types::node::NodeID;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::File;
use std::io::{self, BufWriter};
use std::path::PathBuf;

type Msg = Message<TestScheme, TestCommand>;

pub struct NodeSnapshot {
    pub view: View,
    pub leader: NodeID,
    pub proposed: bool,
    pub high_qc: QuorumCertificate<TestScheme, hotstuff_consensus::types::vote::Vote<TestScheme>>,
    pub lock: QuorumCertificate<TestScheme, hotstuff_consensus::types::vote::Vote<TestScheme>>,
    pub voted_view: View,
    pub exec_height: u64,
    pub exec_hash: TestDigest,
    pub committed: Vec<TestDigest>,
    pub stored: Vec<TestDigest>,
    pub view_timers: Vec<View>,
    pub lock_timer: Option<View>,
}

pub struct JsonTrace {
    path: PathBuf,
    title: String,
    setup: String,
    n: u32,
    crashed: Vec<u32>,
    ids: HashMap<TestDigest, u64>,
    digests: Vec<TestDigest>,
    blocks: BTreeMap<u64, Value>,
    qcs: BTreeSet<(View, u64)>,
    qc_list: Vec<Value>,
    messages: Vec<Value>,
    steps: Vec<Value>,
    tags: BTreeSet<&'static str>,
    prev: Option<BTreeMap<String, (View, u64)>>,
}

impl JsonTrace {
    pub fn new(path: PathBuf, title: &str, setup: &str, n: u32, crashed: Vec<u32>) -> Self {
        Self {
            path,
            title: title.to_owned(),
            setup: setup.to_owned(),
            n,
            crashed,
            ids: HashMap::new(),
            digests: Vec::new(),
            blocks: BTreeMap::new(),
            qcs: BTreeSet::new(),
            qc_list: Vec::new(),
            messages: Vec::new(),
            steps: Vec::new(),
            tags: BTreeSet::new(),
            prev: None,
        }
    }

    pub fn known(&self) -> &[TestDigest] {
        &self.digests
    }

    fn step_index(&self) -> usize {
        self.steps.len().saturating_sub(1)
    }

    fn id(&mut self, digest: TestDigest) -> u64 {
        if let Some(id) = self.ids.get(&digest) {
            return *id;
        }
        let id = self.digests.len() as u64;
        self.ids.insert(digest, id);
        self.digests.push(digest);
        id
    }

    fn qc<T>(&mut self, qc: &QuorumCertificate<TestScheme, T>, view: View, block: TestDigest) -> Value {
        let block = self.id(block);
        let signers: Vec<String> = qc.raw.aggregate().signers().iter().map(|id| u32::from(*id).to_string()).collect();
        let signers = format!("{{{}}}", signers.join(","));
        if self.qcs.insert((view, block)) {
            let step = self.step_index();
            self.qc_list.push(json!({ "block": block, "view": view, "signers": signers, "step": step }));
            self.tags.insert("qc");
        }
        json!({ "block": block, "view": view, "signers": signers })
    }

    pub fn block(&mut self, block: &Block<TestScheme, TestCommand>, view: Option<View>, proposer: Option<NodeID>) {
        let hash = self.id(block.hash);
        if self.blocks.contains_key(&hash) {
            return;
        }
        let parent = if block.height == 0 { -1 } else { self.id(block.parent) as i64 };
        let justify = self.qc(&block.justify, block.justify.view(), block.justify.block());
        let minted = self.step_index();
        self.blocks.insert(
            hash,
            json!({
                "hash": hash, "parent": parent, "height": block.height, "justify": justify, "minted": minted,
                "view": view, "proposer": proposer.map(u32::from),
            }),
        );
    }

    pub fn describe(&mut self, msg: &Msg) -> String {
        match msg {
            Message::ProposalMsg(p) => {
                let (b, j) = (self.id(p.block.hash), self.id(p.block.justify.block()));
                format!("Proposal(v{}, b{b} h{}, QC(b{j}@v{}))", p.view, p.block.height, p.block.justify.view())
            }
            Message::VoteMsg(v) => format!("Vote(v{}, b{})", v.view(), self.id(v.block_hash())),
            Message::WishMsg(w) => format!("Wish(v{})", w.view()),
            Message::StatusMsg { qc, view } => {
                format!("Status(v{view}, QC(b{}@v{}))", self.id(qc.block()), qc.view())
            }
            Message::TCMsg(tc) => format!("TC(v{})", tc.view()),
        }
    }

    pub fn begin(&mut self, time: String, actor: NodeID, action: &str, label: String, received: Option<usize>) {
        self.steps.push(json!({ "action": action, "actor": u32::from(actor), "label": label, "time": time }));
        self.tags.clear();
        match action {
            "view_timeout" | "lock_timeout" => {
                self.tags.insert("timer");
            }
            "synced" => {
                self.tags.insert("sync");
            }
            _ => {}
        }
        if let Some(m) = received {
            self.messages[m]["recv"] = json!(self.step_index());
        }
    }

    pub fn send(&mut self, from: NodeID, to: NodeID, msg: &Msg, delivered: bool) -> usize {
        if let Message::ProposalMsg(p) = msg {
            self.block(&p.block, Some(p.view), Some(from));
        }
        if let Message::StatusMsg { qc, .. } = msg {
            self.qc(qc, qc.view(), qc.block());
        }
        if !delivered {
            self.tags.insert("drop");
        }
        let kind = match msg {
            Message::ProposalMsg(_) => "Proposal",
            Message::VoteMsg(_) => "Vote",
            Message::WishMsg(_) => "Wish",
            Message::StatusMsg { .. } => "Status",
            Message::TCMsg(_) => "TC",
        };
        let text = self.describe(msg);
        let id = self.messages.len();
        let send = self.step_index();
        self.messages.push(json!({
            "id": id, "from": u32::from(from), "to": u32::from(to), "kind": kind, "text": text,
            "channel": "net", "send": send, "recv": null, "dropped": !delivered,
        }));
        id
    }

    pub fn sync_requested(&mut self) {
        self.tags.insert("sync");
    }

    pub fn end(&mut self, nodes: BTreeMap<NodeID, NodeSnapshot>) {
        let mut out = serde_json::Map::new();
        let mut now = BTreeMap::new();
        for (id, n) in nodes {
            let key = u32::from(id).to_string();
            now.insert(key.clone(), (n.view, n.exec_height));
            let high_qc = self.qc(&n.high_qc, n.high_qc.view(), n.high_qc.block());
            let lock = self.qc(&n.lock, n.lock.view(), n.lock.block());
            let committed: Vec<u64> = n.committed.iter().map(|d| self.id(*d)).collect();
            let stored: Vec<u64> = n.stored.iter().map(|d| self.id(*d)).collect();
            let exec_hash = self.id(n.exec_hash);
            out.insert(
                key,
                json!({
                    "view": n.view, "leader": u32::from(n.leader), "proposed": n.proposed,
                    "high_qc": high_qc, "lock": lock, "voted_view": n.voted_view,
                    "exec_height": n.exec_height, "exec_hash": exec_hash,
                    "committed": committed, "stored": stored,
                    "view_timers": n.view_timers, "lock_timer": n.lock_timer,
                }),
            );
        }
        if let Some(prev) = &self.prev {
            for (key, (view, height)) in &now {
                if let Some((v, h)) = prev.get(key) {
                    if v != view {
                        self.tags.insert("view");
                    }
                    if h != height {
                        self.tags.insert("commit");
                    }
                }
            }
        }
        self.prev = Some(now);
        let tags: Vec<&str> = self.tags.iter().copied().collect();
        let step = self.steps.last_mut().expect("end without begin");
        step["nodes"] = Value::Object(out);
        step["tags"] = json!(tags);
    }

    pub fn write(&mut self) -> io::Result<()> {
        let mut running: Vec<u32> = self
            .steps
            .last()
            .and_then(|s| s["nodes"].as_object())
            .map(|nodes| nodes.keys().filter_map(|k| k.parse().ok()).collect())
            .unwrap_or_default();
        running.sort();
        let data = json!({
            "meta": {
                "model": self.title, "source": self.setup, "steps": self.steps.len().saturating_sub(1),
                "running": running, "hosts": (0..self.n).collect::<Vec<_>>(), "faulty": self.crashed,
                "faulty_label": "crashed", "committee_size": self.n, "warnings": [],
            },
            "blocks": self.blocks.values().collect::<Vec<_>>(),
            "qcs": self.qc_list,
            "messages": self.messages,
            "steps": self.steps,
        });
        serde_json::to_writer(BufWriter::new(File::create(&self.path)?), &data)?;
        eprintln!("trace: {}", self.path.display());
        Ok(())
    }
}
