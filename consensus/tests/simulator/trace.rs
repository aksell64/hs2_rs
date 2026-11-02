use hotstuff_consensus::mocks::scheme::{TestCommand, TestScheme};
use hotstuff_consensus::network::message::{Message, Recipient};
use hotstuff_consensus::protocol::leaders::{LeaderPolicy, RoundRobin};
use hotstuff_consensus::sm::Effects;
use hotstuff_consensus::timer::service::TimerCommand;
use hotstuff_consensus::types::EpochTimeouts;
use hotstuff_consensus::types::View;
use hotstuff_consensus::types::clock::Clock;
use hotstuff_types::node::NodeID;
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Debug, Display};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

const TRACE_DIR: &str = concat!(env!("CARGO_TARGET_TMPDIR"), "/traces");
const TABLE_HEADER: &str = "| t | node | event | effects |";
const VIEWS_HEADER: &str = "## Views";

thread_local! {
    static RUNS: RefCell<BTreeMap<String, usize>> = const { RefCell::new(BTreeMap::new()) };
    static LAST: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

pub struct Trace {
    path: PathBuf,
    head: String,
    events: Vec<String>,
    lanes: Lanes,
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

impl Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "v{} · hqc {} · lock {} · exec {}",
            self.view, self.high_qc, self.locked, self.exec
        )?;
        if self.proposed {
            write!(f, " · proposed")?;
        }
        if self.missing_high_qc_block {
            write!(f, " · ⚠ hqc block missing")?;
        }
        Ok(())
    }
}

impl Trace {
    pub fn create(setup: &str, n: u32, seed: u64) -> io::Result<Self> {
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
            events: Vec::new(),
            lanes: Lanes::new(n),
        })
    }

    pub fn step<CL: Clock + Debug>(
        &mut self,
        now: CL,
        node_id: NodeID,
        event: &str,
        timeout: Option<View>,
        effects: &Effects<TestScheme, TestCommand, CL>,
        snapshot: Option<Snapshot>,
    ) {
        let mut parts = Vec::new();
        for envelope in &effects.net {
            parts.push(format!(
                "send {} → {}",
                describe(&envelope.msg),
                recipient(&envelope.to)
            ));
        }
        for command in &effects.timers {
            parts.push(match command {
                TimerCommand::SetEpochTimers(epoch) => {
                    format!("view timers {}", epoch_timers(epoch))
                }
                TimerCommand::SetLockTimer(timer) => {
                    format!("lock timer v{}@{:?}", timer.view, timer.deadline)
                }
                TimerCommand::ClearLockTimer(view) => format!("clear lock v{view}"),
                TimerCommand::ClearViewTimersBelow(view) => format!("clear view timers < v{view}"),
            });
        }
        for _ in &effects.sync {
            parts.push("sync request".to_owned());
        }
        for block in &effects.committed {
            parts.push(format!("**commit h{}**", block.height));
        }
        let summary = if parts.is_empty() {
            "–".to_owned()
        } else {
            parts.join("; ")
        };
        self.events.push(format!(
            "| {now:?} | {} | {event} | {summary} |",
            node(node_id)
        ));
        self.lanes
            .record(now, node_id, event, timeout, effects, snapshot);
    }

    fn write(&self) -> io::Result<()> {
        let mut out = BufWriter::new(File::create(&self.path)?);
        write!(out, "{}", self.head)?;
        self.lanes.write(&mut out)?;
        writeln!(out, "\n## Events\n")?;
        writeln!(out, "{TABLE_HEADER}")?;
        writeln!(out, "|---:|---|---|---|")?;
        for row in &self.events {
            writeln!(out, "{row}")?;
        }
        out.flush()
    }
}

impl Drop for Trace {
    fn drop(&mut self) {
        if let Err(err) = self.write() {
            eprintln!("trace: failed to write {}: {err}", self.path.display());
        }
    }
}

#[derive(Default)]
struct ViewRow {
    proposal: Option<String>,
    votes: BTreeSet<NodeID>,
    qc: Option<String>,
    timeouts: Vec<String>,
    tc: Option<String>,
    committed: Option<String>,
}

struct Lanes {
    n: u32,
    leaders: RoundRobin,
    rows: Vec<String>,
    states: BTreeMap<NodeID, Snapshot>,
    views: BTreeMap<View, ViewRow>,
    blocks: BTreeMap<[u8; 4], View>,
}

impl Lanes {
    fn new(n: u32) -> Self {
        Self {
            n,
            leaders: RoundRobin::new(n),
            rows: Vec::new(),
            states: BTreeMap::new(),
            views: BTreeMap::new(),
            blocks: BTreeMap::new(),
        }
    }

    fn record<CL: Clock + Debug>(
        &mut self,
        now: CL,
        node_id: NodeID,
        event: &str,
        timeout: Option<View>,
        effects: &Effects<TestScheme, TestCommand, CL>,
        snapshot: Option<Snapshot>,
    ) {
        let at = format!("{now:?}");
        let by = || format!("{} @{at}", node(node_id));
        let mut lines = vec![event.to_owned()];
        for envelope in &effects.net {
            lines.push(format!(
                "↳ send {} → {}",
                describe(&envelope.msg),
                recipient(&envelope.to)
            ));
            match &envelope.msg {
                Message::ProposalMsg(proposal) => {
                    self.blocks.insert(proposal.block.hash.0, proposal.view);
                    self.view(proposal.view)
                        .proposal
                        .get_or_insert_with(|| format!("h{} @{at}", proposal.block.height));
                }
                Message::VoteMsg(vote) => {
                    self.view(vote.view()).votes.insert(node_id);
                }
                Message::TCMsg(tc) => {
                    self.view(tc.view()).tc.get_or_insert_with(by);
                }
                _ => {}
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
            lines.push(format!("↳ **commit h{}**", block.height));
            if let Some(view) = self.blocks.get(&block.hash.0).copied() {
                self.view(view)
                    .committed
                    .get_or_insert_with(|| format!("@{at}"));
            }
        }
        if let Some(view) = timeout {
            self.view(view).timeouts.push(by());
        }
        if let Some(snapshot) = snapshot {
            self.view(snapshot.view);
            if snapshot.high_qc > 0 {
                self.view(snapshot.high_qc).qc.get_or_insert_with(by);
            }
            if self.states.get(&node_id) != Some(&snapshot) {
                lines.push(format!("`{snapshot}`"));
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

    fn view(&mut self, view: View) -> &mut ViewRow {
        self.views.entry(view).or_default()
    }

    fn write(&self, out: &mut impl Write) -> io::Result<()> {
        writeln!(out, "{VIEWS_HEADER}\n")?;
        writeln!(
            out,
            "| view | leader | proposal | votes | QC | timeouts | TC | committed |"
        )?;
        writeln!(out, "|---:|---|---|---|---|---|---|---|")?;
        let empty = ViewRow::default();
        let last = self.views.keys().next_back().copied().unwrap_or(0);
        for view in 1..=last {
            let row = self.views.get(&view).unwrap_or(&empty);
            let votes: Vec<String> = row.votes.iter().copied().map(node).collect();
            writeln!(
                out,
                "| {view} | {} | {} | {} | {} | {} | {} | {} |",
                node(self.leaders.leader(view)),
                or_dash(row.proposal.as_deref()),
                or_dash(Some(votes.join(", ")).as_deref()),
                or_dash(row.qc.as_deref()),
                or_dash(Some(row.timeouts.join(", ")).as_deref()),
                or_dash(row.tc.as_deref()),
                or_dash(row.committed.as_deref()),
            )?;
        }
        let nodes: Vec<String> = (0..self.n).map(|id| node(NodeID::from(id))).collect();
        writeln!(out, "\n## Lanes\n")?;
        writeln!(out, "| t | {} |", nodes.join(" | "))?;
        writeln!(out, "|---:|{}", "---|".repeat(nodes.len()))?;
        for row in &self.rows {
            writeln!(out, "{row}")?;
        }
        Ok(())
    }
}

fn or_dash(value: Option<&str>) -> &str {
    match value {
        Some(value) if !value.is_empty() => value,
        _ => "–",
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
    let header = format!("**Violation:** {violation}\n\n{VIEWS_HEADER}");
    fs::write(path, trace.replacen(VIEWS_HEADER, &header, 1))
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
