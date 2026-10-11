use crate::collector::collector::CollectorEvent::GenericVoteInput;
use crate::collector::collector::{
    Collector, CollectorEvent, CollectorOutput, GenericCollectorOutput, TimeoutCollectorOutput,
};
use crate::consensus::consensus::ConsensusEvent::ReceivedProposal;
use crate::consensus::consensus::{Consensus, ConsensusEvent, ConsensusOutput, ProposeRequest};
use crate::network::message::{Envelope, Message, Recipient};
use crate::pacemaker::pacemaker::{Pacemaker, PacemakerEvent, PacemakerOutput};
use crate::storage::kv::{KVError, Storage};
use crate::types::View;
use crate::types::block::{Block, Command, Proposal};
use crate::types::certificate::{CertifiedBlock, CertifiedView};
use crate::types::vote::Vote;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::collections::BTreeMap;

pub enum DriverEvent<S: Scheme, C: Command> {
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
    NewStatus {
        sender: NodeID,
        qc: CertifiedBlock<S>,
        view: View,
    },
    NewTC(CertifiedView<S>),
    ViewTimerFired(View),
    LockTimerFired(View),
    EnterView(View),
    BlockSynced(Block<S, C>),
}

pub enum DriverOutput<S: Scheme, C: Command> {
    NewView(View),
    SetViewTimer(View),
    SetLockTimer(View),
    ClearLockTimer(View),
    Replay(DriverEvent<S, C>),
}

pub struct DriverResult<S: Scheme, C: Command> {
    pub net: Vec<Envelope<S, C>>,
    pub out: Vec<DriverOutput<S, C>>,
}

enum Task<S: Scheme, C: Command> {
    Pacemaker(PacemakerEvent<S>),
    Consensus(ConsensusEvent<S, C>),
    Collector(CollectorEvent<S>),
    StoreBlock(Block<S, C>),
    Commit(Vec<Block<S, C>>),
    ProposalOrBuffer(Proposal<S, C>),
    ReleasePending(View),
    Send(Envelope<S, C>),
}

enum TaskOut<S: Scheme, C: Command> {
    Pacemaker(PacemakerOutput<S>),
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

pub struct Driver<S: Scheme, C: Command> {
    consensus: Consensus<S, C>,
    pacemaker: Pacemaker<S>,
    collector: Collector<S>,
    pending: BTreeMap<View, Vec<(NodeID, Proposal<S, C>)>>,
}

impl<S: Scheme, C: Command> Driver<S, C> {
    pub fn apply<DB: Storage<S, C>>(
        &mut self,
        storage: &mut DB,
        evt: DriverEvent<S, C>,
    ) -> Result<DriverResult<S, C>, KVError> {
        let stack = self.prepare_tasks_from_event(evt);
        self.apply_nested(storage, stack)
    }

    fn prepare_tasks_from_event(&mut self, event: DriverEvent<S, C>) -> TasksStack<S, C> {
        let mut stack = TasksStack::new();
        let tasks = self.tasks_from_event(event);
        stack.push(tasks);
        stack
    }

    fn tasks_from_event(&self, event: DriverEvent<S, C>) -> Vec<Task<S, C>> {
        match event {
            DriverEvent::NewProposal { sender, proposal } => {
                let proposed_view_leader = self.pacemaker.leader(proposal.view);
                if sender != proposed_view_leader {
                    return vec![];
                }
                vec![Task::Consensus(ReceivedProposal(proposal))]
            }

            DriverEvent::NewVote { sender, vote } => {
                if !self.pacemaker.is_leader(vote.view + 1) {
                    return vec![];
                }
                vec![Task::Collector(GenericVoteInput { sender, vote })]
            }

            DriverEvent::NewWish { sender, view } => {
                vec![Task::Pacemaker(PacemakerEvent::ReceiveWish {
                    sender,
                    view,
                })]
            }

            DriverEvent::NewStatus { sender, qc, view } => {
                vec![Task::Pacemaker(PacemakerEvent::ReceiveStatus {
                    sender,
                    qc,
                    view,
                })]
            }

            DriverEvent::NewTC(tc) => vec![Task::Pacemaker(PacemakerEvent::TimeoutQC(tc))],

            DriverEvent::ViewTimerFired(view) => {
                vec![Task::Pacemaker(PacemakerEvent::TimerExpiredView(view))]
            }

            DriverEvent::LockTimerFired(view) => {
                vec![Task::Pacemaker(PacemakerEvent::LockTimerExpired(view))]
            }

            DriverEvent::EnterView(view) => vec![
                Task::Pacemaker(PacemakerEvent::EnterViewRequested(view)),
                Task::ReleasePending(view),
            ],

            DriverEvent::BlockSynced(_) => vec![],
        }
    }

    fn apply_nested<DB: Storage<S, C>>(
        &mut self,
        storage: &mut DB,
        mut stack: TasksStack<S, C>,
    ) -> Result<DriverResult<S, C>, KVError> {
        let mut result = DriverResult {
            net: Vec::new(),
            out: Vec::new(),
        };
        while let Some(task) = stack.pop() {
            if let Some(out) = self.process_input(storage, task, &mut result)? {
                stack.push(self.process_out(out, &mut result));
            }
        }
        Ok(result)
    }

    fn process_input<DB: Storage<S, C>>(
        &mut self,
        storage: &mut DB,
        task: Task<S, C>,
        result: &mut DriverResult<S, C>,
    ) -> Result<Option<TaskOut<S, C>>, KVError> {
        match task {
            Task::Pacemaker(evt) => Ok(self.pacemaker.apply(evt).map(TaskOut::Pacemaker)),
            Task::Consensus(evt) => {
                let cur_view = self.pacemaker.get_current_view();
                let high_qc = self.pacemaker.get_high_qc();
                Ok(self
                    .consensus
                    .apply(storage, cur_view, high_qc, evt)?
                    .map(TaskOut::Consensus))
            }
            Task::Collector(evt) => Ok(Some(TaskOut::Collector(self.collector.apply(evt)))),
            Task::StoreBlock(block) => {
                storage.insert_block(block)?;
                Ok(None)
            }
            Task::Commit(blocks) => {
                for block in blocks {
                    storage.commit_block(block)?;
                }
                Ok(None)
            }
            Task::ProposalOrBuffer(proposal) => Ok(None),
            Task::ReleasePending(view) => {
                let released = self.pending.remove(&view).unwrap_or_default();
                result
                    .out
                    .extend(released.into_iter().map(|(sender, proposal)| {
                        DriverOutput::Replay(DriverEvent::NewProposal { sender, proposal })
                    }));
                Ok(None)
            }
            Task::Send(envelope) => {
                result.net.push(envelope);
                Ok(None)
            }
        }
    }

    fn process_out(
        &mut self,
        output: TaskOut<S, C>,
        result: &mut DriverResult<S, C>,
    ) -> Vec<Task<S, C>> {
        match output {
            TaskOut::Pacemaker(out) => self.process_pacemaker_out(out, result),
            TaskOut::Consensus(out) => self.process_consensus_out(out, result),
            TaskOut::Collector(out) => self.process_collector_out(out, result),
        }
    }

    fn process_pacemaker_out(
        &mut self,
        pacemaker_out: PacemakerOutput<S>,
        result: &mut DriverResult<S, C>,
    ) -> Vec<Task<S, C>> {
        match pacemaker_out {
            PacemakerOutput::HighQCUpdated(qc) => {
                vec![Task::Pacemaker(PacemakerEvent::HighQCApplied(qc))]
            }
            PacemakerOutput::AdvanceView(view) => {
                vec![Task::Pacemaker(PacemakerEvent::AdvanceViewRequested(view))]
            }
            PacemakerOutput::CancelLockTimer { view, next } => {
                result.out.push(DriverOutput::ClearLockTimer(view));
                vec![Task::Pacemaker(PacemakerEvent::LockTimerCanceled(next))]
            }
            PacemakerOutput::StartTimerView(view) => {
                result.out.push(DriverOutput::SetViewTimer(view));
                vec![Task::Pacemaker(PacemakerEvent::ViewTimerStarted(view))]
            }
            PacemakerOutput::EnterNewView(view) => {
                result.out.push(DriverOutput::NewView(view));
                vec![]
            }
            PacemakerOutput::StartLockTimer(view) => {
                result.out.push(DriverOutput::SetLockTimer(view));
                vec![]
            }
            PacemakerOutput::ProposeNow(view) => {
                vec![Task::Consensus(ConsensusEvent::MakeProposal(
                    self.propose_request(view),
                ))]
            }
            PacemakerOutput::SendStatus { to, qc, view } => vec![Task::Send(Envelope {
                to: Recipient::Node(to),
                msg: Message::StatusMsg { qc, view },
            })],
            PacemakerOutput::SendWish { to, view } => to
                .into_iter()
                .map(|node| {
                    Task::Send(Envelope {
                        to: Recipient::Node(node),
                        msg: Message::WishMsg(view),
                    })
                })
                .collect(),
            PacemakerOutput::ReceiveWishIsLeader { sender, view } => {
                vec![Task::Collector(CollectorEvent::WishInput { sender, view })]
            }
            PacemakerOutput::BroadcastTC(tc) => vec![
                Task::Pacemaker(PacemakerEvent::AdvanceViewRequested(tc.view)),
                Task::Send(Envelope {
                    to: Recipient::All,
                    msg: Message::TCMsg(tc),
                }),
            ],
            PacemakerOutput::SendTCToLeader { to, tc } => {
                let advance = Task::Pacemaker(PacemakerEvent::AdvanceViewRequested(tc.view));
                let relays = to.into_iter().map(|node| {
                    Task::Send(Envelope {
                        to: Recipient::Node(node),
                        msg: Message::TCMsg(tc.clone()),
                    })
                });
                std::iter::once(advance).chain(relays).collect()
            }
            PacemakerOutput::StatusIsLeader { qc, .. } => {
                vec![Task::Pacemaker(PacemakerEvent::ProcessQC(qc))]
            }
        }
    }

    fn process_consensus_out(
        &mut self,
        consensus_out: ConsensusOutput<S, C>,
        _: &mut DriverResult<S, C>,
    ) -> Vec<Task<S, C>> {
        match consensus_out {
            ConsensusOutput::ReceiveQC { qc, proposal } => vec![
                Task::Pacemaker(PacemakerEvent::ProcessQC(qc)),
                Task::ProposalOrBuffer(proposal),
            ],
            ConsensusOutput::ConfirmVote { vote, block } => {
                let leader = self.pacemaker.leader(vote.view + 1);
                vec![
                    Task::StoreBlock(block.clone()),
                    Task::Consensus(ConsensusEvent::Update(block)),
                    Task::Send(Envelope {
                        to: Recipient::Node(leader),
                        msg: Message::VoteMsg(vote),
                    }),
                ]
            }
            ConsensusOutput::RejectVote(block) => vec![
                Task::StoreBlock(block.clone()),
                Task::Consensus(ConsensusEvent::Update(block)),
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
        _: &mut DriverResult<S, C>,
    ) -> Vec<Task<S, C>> {
        match collector_out {
            CollectorOutput::GenericOutput(GenericCollectorOutput::Ready(qc)) => {
                vec![Task::Pacemaker(PacemakerEvent::ProcessQC(qc))]
            }
            CollectorOutput::GenericOutput(
                GenericCollectorOutput::AlreadyReady(_) | GenericCollectorOutput::NotReady,
            ) => {
                vec![]
            }
            CollectorOutput::TimeoutOutput(TimeoutCollectorOutput::TimeoutReady(tc)) => {
                vec![Task::Pacemaker(PacemakerEvent::TimeoutQC(tc))]
            }
            CollectorOutput::TimeoutOutput(
                TimeoutCollectorOutput::TimeoutAlreadyReady(_)
                | TimeoutCollectorOutput::TimeoutNotReady,
            ) => vec![],
        }
    }

    fn propose_request(&mut self, view: View) -> ProposeRequest<S, C> {
        todo!()
    }
}
