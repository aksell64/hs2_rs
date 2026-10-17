use crate::network::message::{Envelope, Message, Recipient};
use crate::protocol::collector::CollectorInput::GenericVoteInput;
use crate::protocol::collector::{
    Collector, CollectorInput, CollectorOutput, GenericCollectorOutput, TimeoutCollectorOutput,
};
use crate::protocol::consensus::{Consensus, ConsensusInput, ConsensusOutput, ProposeRequest};
use crate::protocol::effects::Effect;
use crate::protocol::pacemaker::{Pacemaker, PacemakerInput, PacemakerOutput, Status};
use crate::protocol::timeouts::TimeoutPolicy;
use crate::storage::{KVError, ReadWriteStorage};
use crate::timer::service::TimerCommand;
use crate::types::View;
use crate::types::block::{Block, Command, Proposal};
use crate::types::certificate::TimeoutQC;
use crate::types::clock::{Clock, Source};
use crate::types::mempool::Mempool;
use crate::types::vote::Vote;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::collections::BTreeMap;

pub enum DriverInput<S: Scheme, C: Command> {
    NewProposal {
        sender: NodeID,
        proposal: Proposal<S, C>,
    },
    NewVote {
        sender: NodeID,
        vote: Vote<S>,
    },
    NewWish {
        sender: NodeID,
        view: View,
    },
    NewStatus(Status<S>),
    NewTC(TimeoutQC<S>),
    ViewTimerFired(View),
    LockTimerFired(View),
    BlockSynced(Block<S, C>),
}

pub enum DriverOutput<S: Scheme, C: Command> {
    Replay(DriverInput<S, C>),
}

pub struct DriverResult<S: Scheme, C: Command, CL: Clock> {
    pub net: Vec<Envelope<S, C>>,
    pub timers: Vec<TimerCommand<CL>>,
    pub out: Vec<DriverOutput<S, C>>,
    pub committed: Vec<Block<S, C>>,
}

enum Task<S: Scheme, C: Command> {
    Pacemaker(PacemakerInput<S>),
    Consensus(ConsensusInput<S, C>),
    Collector(CollectorInput<S>),
    StoreBlock(Block<S, C>),
    Commit(Vec<Block<S, C>>),
    ProposalOrBuffer(Proposal<S, C>),
    Send(Envelope<S, C>),
}

enum TaskOut<S: Scheme, C: Command> {
    Pacemaker(PacemakerOutput),
    Consensus(ConsensusOutput<S, C>),
    Collector(CollectorOutput<S>),
}

struct TasksStack<S: Scheme, C: Command>(Vec<Task<S, C>>);

impl<S: Scheme, C: Command> TasksStack<S, C> {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn push<I>(&mut self, tasks: I)
    where
        I: IntoIterator<Item = Task<S, C>, IntoIter: DoubleEndedIterator>,
    {
        self.0.extend(tasks.into_iter().rev());
    }

    fn pop(&mut self) -> Option<Task<S, C>> {
        self.0.pop()
    }
}

pub struct Driver<S: Scheme, C: Command, TP: TimeoutPolicy, TS: Source<Clock = TP::Clock>> {
    consensus: Consensus<S>,
    pacemaker: Pacemaker<S, TP, TS>,
    collector: Collector<S>,
    pending: BTreeMap<View, Vec<(NodeID, Proposal<S, C>)>>,
}

impl<S: Scheme, C: Command, TP: TimeoutPolicy, TS: Source<Clock = TP::Clock>> Driver<S, C, TP, TS> {
    pub fn apply<DB: ReadWriteStorage<S, C>, M: Mempool<C>>(
        &mut self,
        storage: &mut DB,
        mempool: &mut M,
        evt: DriverInput<S, C>,
    ) -> Result<DriverResult<S, C, TP::Clock>, KVError> {
        let stack = self.prepare_tasks_from_event(evt);
        self.apply_nested(storage, mempool, stack)
    }

    fn prepare_tasks_from_event(&mut self, event: DriverInput<S, C>) -> TasksStack<S, C> {
        let mut stack = TasksStack::new();
        let tasks = self.tasks_from_event(event);
        stack.push(tasks);
        stack
    }

    fn tasks_from_event(&self, event: DriverInput<S, C>) -> Vec<Task<S, C>> {
        match event {
            DriverInput::NewProposal { sender, proposal } => {
                if sender != self.pacemaker.leader(proposal.view)
                    || !self.consensus.is_valid_qc(&proposal.block.justify)
                    || !self.consensus.verify_block_hash(&proposal.block)
                {
                    return vec![];
                }
                let justify = proposal.block.justify.clone();
                vec![
                    Task::Pacemaker(PacemakerInput::ReceivedQC(justify)),
                    Task::ProposalOrBuffer(proposal),
                ]
            }

            DriverInput::NewVote { sender, vote } => {
                if !self.pacemaker.is_leader(vote.view + 1) {
                    return vec![];
                }
                vec![Task::Collector(GenericVoteInput { sender, vote })]
            }

            DriverInput::NewWish { sender, view } => {
                if !self.pacemaker.is_epoch_leader(view) {
                    return vec![];
                }
                vec![Task::Collector(CollectorInput::WishInput { sender, view })]
            }

            DriverInput::NewStatus(status) => {
                vec![Task::Pacemaker(PacemakerInput::ReceivedStatus(status))]
            }

            DriverInput::NewTC(tc) => vec![Task::Pacemaker(PacemakerInput::ReceivedTC(tc))],

            DriverInput::ViewTimerFired(view) => {
                vec![Task::Pacemaker(PacemakerInput::ViewTimerExpired(view))]
            }

            DriverInput::LockTimerFired(view) => {
                vec![Task::Pacemaker(PacemakerInput::LockTimerExpired(view))]
            }

            DriverInput::BlockSynced(_) => vec![],
        }
    }

    fn apply_nested<DB: ReadWriteStorage<S, C>, M: Mempool<C>>(
        &mut self,
        storage: &mut DB,
        mempool: &mut M,
        mut stack: TasksStack<S, C>,
    ) -> Result<DriverResult<S, C, TP::Clock>, KVError> {
        let mut result = DriverResult {
            net: Vec::new(),
            timers: Vec::new(),
            out: Vec::new(),
            committed: Vec::new(),
        };
        while let Some(task) = stack.pop() {
            if let Some(out) = self.process_input(storage, task, &mut result)? {
                stack.push(self.process_out(mempool, out, &mut result));
            }
        }
        Ok(result)
    }

    fn process_input<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        task: Task<S, C>,
        result: &mut DriverResult<S, C, TP::Clock>,
    ) -> Result<Option<TaskOut<S, C>>, KVError> {
        match task {
            Task::Pacemaker(evt) => {
                let old_view = storage.get_cur_view()?;
                let called = self.pacemaker.apply(storage, evt)?;
                for effect in called.effects {
                    match effect {
                        Effect::Send(envelope) => result.net.push(envelope),
                        Effect::Timer(command) => result.timers.push(command),
                    }
                }
                let new_view = storage.get_cur_view()?;
                if new_view > old_view {
                    self.release_pending(new_view, result);
                }
                match called.out {
                    PacemakerOutput::NoPacemakerOutput => Ok(None),
                    out => Ok(Some(TaskOut::Pacemaker(out))),
                }
            }
            Task::Consensus(evt) => {
                let cur_view = storage.get_cur_view()?;
                let high_qc = storage.get_high_qc()?;
                Ok(self
                    .consensus
                    .apply(storage, cur_view, &high_qc, evt)?
                    .map(TaskOut::Consensus))
            }
            Task::Collector(evt) => Ok(Some(TaskOut::Collector(self.collector.apply(evt)))),
            Task::StoreBlock(block) => {
                storage.insert_block(&block)?;
                Ok(None)
            }
            Task::Commit(blocks) => {
                for block in blocks {
                    storage.commit_block(&block)?;
                    result.committed.push(block);
                }
                Ok(None)
            }
            Task::ProposalOrBuffer(proposal) => Ok(None),
            Task::Send(envelope) => {
                result.net.push(envelope);
                Ok(None)
            }
        }
    }

    fn process_out<M: Mempool<C>>(
        &mut self,
        mempool: &mut M,
        output: TaskOut<S, C>,
        result: &mut DriverResult<S, C, TP::Clock>,
    ) -> Vec<Task<S, C>> {
        match output {
            TaskOut::Pacemaker(out) => self.process_pacemaker_out(mempool, out, result),
            TaskOut::Consensus(out) => self.process_consensus_out(out, result),
            TaskOut::Collector(out) => self.process_collector_out(out, result),
        }
    }

    fn release_pending(&mut self, view: View, result: &mut DriverResult<S, C, TP::Clock>) {
        let released = self.pending.remove(&view).unwrap_or_default();
        result
            .out
            .extend(released.into_iter().map(|(sender, proposal)| {
                DriverOutput::Replay(DriverInput::NewProposal { sender, proposal })
            }));
    }

    fn process_pacemaker_out<M: Mempool<C>>(
        &mut self,
        mempool: &mut M,
        pacemaker_out: PacemakerOutput,
        _: &mut DriverResult<S, C, TP::Clock>,
    ) -> Vec<Task<S, C>> {
        match pacemaker_out {
            PacemakerOutput::ProposeNow(view) => {
                let request = Self::propose_request(mempool, view);
                vec![Task::Consensus(ConsensusInput::MakeProposal(request))]
            }
            PacemakerOutput::NoPacemakerOutput => vec![],
        }
    }

    fn process_consensus_out(
        &mut self,
        consensus_out: ConsensusOutput<S, C>,
        _: &mut DriverResult<S, C, TP::Clock>,
    ) -> Vec<Task<S, C>> {
        match consensus_out {
            ConsensusOutput::ConfirmVote { vote, block } => {
                let leader = self.pacemaker.leader(vote.view + 1);
                vec![
                    Task::StoreBlock(block.clone()),
                    Task::Consensus(ConsensusInput::Update(block)),
                    Task::Send(Envelope {
                        to: Recipient::Node(leader),
                        msg: Message::VoteMsg(vote),
                    }),
                ]
            }
            ConsensusOutput::RejectVote(block) => vec![
                Task::StoreBlock(block.clone()),
                Task::Consensus(ConsensusInput::Update(block)),
            ],
            ConsensusOutput::Commit(blocks) => vec![Task::Commit(blocks)],
            ConsensusOutput::Propose(proposal) => vec![Task::Send(Envelope {
                to: Recipient::All,
                msg: Message::ProposalMsg(proposal),
            })],
        }
    }

    fn process_collector_out(
        &mut self,
        collector_out: CollectorOutput<S>,
        _: &mut DriverResult<S, C, TP::Clock>,
    ) -> Vec<Task<S, C>> {
        match collector_out {
            CollectorOutput::GenericOutput(GenericCollectorOutput::Ready(qc)) => {
                vec![Task::Pacemaker(PacemakerInput::ReceivedQC(qc))]
            }
            CollectorOutput::GenericOutput(
                GenericCollectorOutput::AlreadyReady(_) | GenericCollectorOutput::NotReady,
            ) => {
                vec![]
            }
            CollectorOutput::TimeoutOutput(TimeoutCollectorOutput::TimeoutReady(tc)) => {
                vec![Task::Pacemaker(PacemakerInput::ReceivedTC(tc))]
            }
            CollectorOutput::TimeoutOutput(
                TimeoutCollectorOutput::TimeoutAlreadyReady(_)
                | TimeoutCollectorOutput::TimeoutNotReady,
            ) => vec![],
        }
    }

    fn propose_request<M: Mempool<C>>(mempool: &mut M, view: View) -> ProposeRequest<C> {
        ProposeRequest {
            cmd: mempool.next_command(),
            view,
        }
    }
}
