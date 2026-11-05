use crate::simulator::viewer::{self, ViewerTrace};
use hotstuff_consensus::mocks::scheme::{TestCommand, TestScheme};
use hotstuff_consensus::network::message::{Message, Recipient};
use hotstuff_consensus::sm::Effects;
use hotstuff_consensus::timer::service::TimerCommand;
use hotstuff_consensus::types::EpochTimeouts;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::clock::Clock;
use hotstuff_types::node::NodeID;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

const TRACE_DIR: &str = concat!(env!("CARGO_TARGET_TMPDIR"), "/traces");
const LANES_HEADER: &str = "## Lanes";

thread_local! {
    static RUNS: RefCell<BTreeMap<String, usize>> = const { RefCell::new(BTreeMap::new()) };
    static LAST: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub struct Trace {
    path: PathBuf,
    head: String,
    notes: Vec<String>,
    lanes: Lanes,
    pub viewer: ViewerTrace,
}

#[derive(PartialEq)]
pub struct Snapshot {
    pub view: View,
    pub high_qc: View,
    pub locked: View,
    pub exec: u64,
    pub proposed: bool,
    pub missing_high_qc_block: bool,
}

impl Snapshot {
    fn describe(&self, prev: Option<&Snapshot>) -> String {
        let was = |prefix: &str, new: u64, get: fn(&Snapshot) -> u64| match prev.map(get) {
            Some(old) if old != new => format!(" (old {prefix}{old})"),
            _ => String::new(),
        };
        let mut line = format!(
            "v{}{} · hqc {}{} · lock {}{} · exec {}{}",
            self.view,
            was("v", self.view, |s| s.view),
            self.high_qc,
            was("", self.high_qc, |s| s.high_qc),
            self.locked,
            was("", self.locked, |s| s.locked),
            self.exec,
            was("", self.exec, |s| s.exec),
        );
        if self.proposed {
            line.push_str(" · proposed");
        }
        if self.missing_high_qc_block {
            line.push_str(" · ⚠ hqc block missing");
        } else if prev.is_some_and(|s| s.missing_high_qc_block) {
            line.push_str(" · hqc block present");
        }
        line
    }
}

impl Trace {
    pub fn create(
        setup: &str,
        n: u32,
        seed: u64,
        byzantine: &BTreeSet<NodeID>,
        crashed: &BTreeSet<NodeID>,
    ) -> io::Result<Self> {
        let test = std::thread::current()
            .name()
            .and_then(|name| name.rsplit("::").next())
            .unwrap_or("trace")
            .to_owned();
        let name = format!("{test}-{seed}");
        let run = RUNS.with_borrow_mut(|runs| {
            let count = runs.entry(name.clone()).or_insert(0);
            *count += 1;
            *count - 1
        });
        let title = match run {
            0 => name,
            run => format!("{name}-{run}"),
        };
        fs::create_dir_all(TRACE_DIR)?;
        let path = PathBuf::from(TRACE_DIR).join(format!("{title}.md"));
        eprintln!("trace: {}", path.display());
        LAST.set(Some(path.clone()));
        Ok(Self {
            path,
            head: format!("# {title}\n\n{setup}\n\n"),
            notes: Vec::new(),
            lanes: Lanes::new(n),
            viewer: ViewerTrace::new(&title, setup, n, byzantine, crashed),
        })
    }

    pub fn note(&mut self, note: String) {
        self.notes.push(note);
    }

    pub fn mark<CL: Debug>(&mut self, at: CL, label: &str) {
        self.viewer.mark(&at, label);
        self.lanes.mark(at, label);
    }

    pub fn step<CL: Clock + Debug>(
        &mut self,
        now: CL,
        node_id: NodeID,
        event: &str,
        effects: &Effects<TestScheme, TestCommand, CL>,
        snapshot: Option<Snapshot>,
    ) {
        self.lanes.record(now, node_id, event, effects, snapshot);
    }

    fn write(&self) -> io::Result<()> {
        let mut out = BufWriter::new(File::create(&self.path)?);
        write!(out, "{}", self.head)?;
        for note in &self.notes {
            writeln!(out, "{note}\n")?;
        }
        self.lanes.write(&mut out)?;
        out.flush()?;
        self.viewer
            .write(&self.path.with_extension("json"), &self.notes)
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        if let Err(err) = self.write() {
            eprintln!("trace: failed to write {}: {err}", self.path.display());
        }
    }
}

struct Lanes {
    n: u32,
    rows: Vec<String>,
    states: BTreeMap<NodeID, Snapshot>,
    block_views: BTreeMap<[u8; 4], View>,
}

impl Lanes {
    fn new(n: u32) -> Self {
        Self {
            n,
            rows: Vec::new(),
            states: BTreeMap::new(),
            block_views: BTreeMap::new(),
        }
    }

    fn mark<CL: Debug>(&mut self, at: CL, label: &str) {
        let cells = vec![format!("**── {label} ──**"); self.n as usize];
        self.rows
            .push(format!("| **{at:?}** | {} |", cells.join(" | ")));
    }

    fn record<CL: Clock + Debug>(
        &mut self,
        now: CL,
        node_id: NodeID,
        event: &str,
        effects: &Effects<TestScheme, TestCommand, CL>,
        snapshot: Option<Snapshot>,
    ) {
        let at = format!("{now:?}");
        let mut lines = vec![event.to_owned()];
        for envelope in &effects.net {
            lines.push(format!(
                "↳ send {} → {}",
                describe(&envelope.msg),
                recipient(&envelope.to)
            ));
            if let Message::ProposalMsg(proposal) = &envelope.msg {
                self.block_views
                    .insert(proposal.block.hash.0, proposal.view);
            }
        }
        for command in &effects.timers {
            match command {
                TimerCommand::SetEpochTimers(epoch) => {
                    lines.push(format!("↳ view timers {}", epoch_timers(epoch)));
                }
                TimerCommand::SetLockTimer(timer) => {
                    lines.push(format!("↳ lock timer v{}@{:?}", timer.view, timer.deadline));
                }
                TimerCommand::ClearLockTimer(_) | TimerCommand::ClearViewTimersBelow(_) => {}
            }
        }
        for _ in &effects.sync {
            lines.push("↳ sync request".to_owned());
        }
        for block in &effects.committed {
            let view = self
                .block_views
                .get(&block.hash.0)
                .map_or(String::new(), |view| format!(" (v{view})"));
            lines.push(format!("↳ **commit h{}{view}**", block.height));
        }
        if let Some(snapshot) = snapshot {
            let prev = self.states.get(&node_id);
            if prev != Some(&snapshot) {
                lines.push(format!("`{}`", snapshot.describe(prev)));
                self.states.insert(node_id, snapshot);
            }
        }
        let cells: Vec<String> = (0..self.n)
            .map(|id| {
                if NodeID::from(id) == node_id {
                    lines.join("<br>")
                } else {
                    String::new()
                }
            })
            .collect();
        self.rows.push(format!("| {at} | {} |", cells.join(" | ")));
    }

    fn write(&self, out: &mut impl Write) -> io::Result<()> {
        let nodes: Vec<String> = (0..self.n).map(|id| node(NodeID::from(id))).collect();
        writeln!(out, "{LANES_HEADER}\n")?;
        writeln!(out, "| t | {} |", nodes.join(" | "))?;
        writeln!(out, "|---:|{}", "---|".repeat(nodes.len()))?;
        for row in &self.rows {
            writeln!(out, "{row}")?;
        }
        Ok(())
    }
}

fn recipient(to: &Recipient) -> String {
    match to {
        Recipient::All => "all".to_owned(),
        Recipient::Node(to) => node(*to),
    }
}

fn epoch_timers<CL: Clock + Debug>(epoch: &EpochTimeouts<CL>) -> String {
    let timers: Vec<String> = epoch
        .timeouts
        .iter()
        .map(|timer| format!("v{}@{:?}", timer.view, timer.deadline))
        .collect();
    timers.join(", ")
}

pub fn record_violation(violation: &str) -> io::Result<()> {
    let Some(path) = LAST.take() else {
        return Ok(());
    };
    let trace = fs::read_to_string(&path)?;
    let header = format!("**Violation:** {violation}\n\n{LANES_HEADER}");
    fs::write(&path, trace.replacen(LANES_HEADER, &header, 1))?;
    viewer::record_violation(&path.with_extension("json"), violation)
}

pub fn node(id: NodeID) -> String {
    format!("n{}", u32::from(id))
}

pub fn describe(msg: &Message<TestScheme, TestCommand>) -> String {
    match msg {
        Message::ProposalMsg(proposal) => {
            format!("Proposal(v{}, h{})", proposal.view, proposal.block.height)
        }
        Message::VoteMsg(vote) => format!("Vote(v{})", vote.view()),
        Message::WishMsg(wish) => format!("Wish(v{})", wish.view()),
        Message::StatusMsg { view, .. } => format!("Status(v{view})"),
        Message::TCMsg(tc) => format!("TC(v{})", tc.view()),
    }
}
