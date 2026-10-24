use hotstuff_consensus::mocks::scheme::{TestCommand, TestScheme};
use hotstuff_consensus::network::message::{Message, Recipient};
use hotstuff_consensus::sm::Effects;
use hotstuff_consensus::timer::service::TimerCommand;
use hotstuff_consensus::types::clock::Clock;
use hotstuff_types::node::NodeID;
use std::cell::Cell;
use std::fmt::Debug;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;

const TRACE_DIR: &str = concat!(env!("CARGO_TARGET_TMPDIR"), "/traces");

thread_local! {
    static RUNS: Cell<usize> = const { Cell::new(0) };
}

pub struct Trace {
    out: BufWriter<File>,
}

impl Trace {
    pub fn create(setup: &str) -> io::Result<Self> {
        let test = std::thread::current()
            .name()
            .and_then(|name| name.rsplit("::").next())
            .unwrap_or("trace")
            .to_owned();
        let run = RUNS.replace(RUNS.get() + 1);
        let title = match run {
            0 => test,
            run => format!("{test}-{run}"),
        };
        fs::create_dir_all(TRACE_DIR)?;
        let path = PathBuf::from(TRACE_DIR).join(format!("{title}.md"));
        let mut out = BufWriter::new(File::create(&path)?);
        writeln!(out, "# {title}\n\n{setup}\n")?;
        writeln!(out, "| t | node | event | effects |")?;
        writeln!(out, "|---:|---|---|---|")?;
        eprintln!("trace: {}", path.display());
        Ok(Self { out })
    }

    pub fn step<CL: Clock + Debug>(
        &mut self,
        now: CL,
        node_id: NodeID,
        event: &str,
        effects: &Effects<TestScheme, TestCommand, CL>,
    ) -> io::Result<()> {
        let mut parts = Vec::new();
        for envelope in &effects.net {
            let to = match envelope.to {
                Recipient::All => "all".to_owned(),
                Recipient::Node(to) => node(to),
            };
            parts.push(format!("{} → {to}", describe(&envelope.msg)));
        }
        for command in &effects.timers {
            parts.push(match command {
                TimerCommand::SetEpochTimers(epoch) => {
                    let timers: Vec<String> = epoch
                        .timeouts
                        .iter()
                        .map(|timer| format!("v{}@{:?}", timer.view, timer.deadline))
                        .collect();
                    format!("view timers {}", timers.join(", "))
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
        let effects = if parts.is_empty() {
            "–".to_owned()
        } else {
            parts.join("; ")
        };
        writeln!(self.out, "| {now:?} | {} | {event} | {effects} |", node(node_id))
    }
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
