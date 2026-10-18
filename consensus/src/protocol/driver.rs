use crate::network::message::{Envelope, Message, Recipient};
use crate::protocol::collector::CollectorInput::GenericVoteInput;
use crate::protocol::collector::{
    Collector, CollectorInput, CollectorOutput, GenericCollectorOutput, TimeoutCollectorOutput,
};
use crate::protocol::consensus::{Consensus, ConsensusInput, ConsensusOutput, ProposeRequest};
use crate::protocol::effects::Effect;
use crate::protocol::leaders::LeaderPolicy;
use crate::protocol::pacemaker::{Pacemaker, PacemakerInput, PacemakerOutput, Status};
use crate::protocol::timeouts::TimeoutPolicy;
use crate::storage::{KVError, ReadWriteStorage};
use crate::timer::service::TimerCommand;
use crate::types::View;
use crate::types::block::{Block, Command, Proposal};
use crate::types::certificate::{SignedVote, SignedWish, TimeoutQC};
use crate::types::clock::{Clock, Source};
use crate::types::crypto::Notary;
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
    NewVote(SignedVote<S>),
    NewWish(SignedWish<S>),
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
    ProposalOrBuffer {
        sender: NodeID,
        proposal: Proposal<S, C>,
    },
    Send(Envelope<S, C>),
}

enum TaskOut<S: Scheme, C: Command> {
    Pacemaker(PacemakerOutput),
    Consensus(ConsensusOutput<S, C>),
    Collector(CollectorOutput<S>),
}

type TaskApply<S, C> = Result<Option<TaskOut<S, C>>, KVError>;

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

pub struct Driver<
    S: Scheme,
    C: Command,
    TP: TimeoutPolicy,
    TS: Source<Clock = TP::Clock>,
    LP: LeaderPolicy,
> {
    notary: Notary<S>,
    consensus: Consensus<S>,
    pacemaker: Pacemaker<S, TP, TS, LP>,
    collector: Collector<S>,
    pending: BTreeMap<View, Vec<(NodeID, Proposal<S, C>)>>,
}

impl<S: Scheme, C: Command, TP: TimeoutPolicy, TS: Source<Clock = TP::Clock>, LP: LeaderPolicy>
    Driver<S, C, TP, TS, LP>
{
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
                self.tasks_from_new_proposal(sender, proposal)
            }
            DriverInput::NewVote(signed_vote) => self.tasks_from_new_vote(signed_vote),
            DriverInput::NewWish(signed_wish) => self.tasks_from_new_wish(signed_wish),
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

    fn tasks_from_new_proposal(&self, sender: NodeID, proposal: Proposal<S, C>) -> Vec<Task<S, C>> {
        if sender != self.pacemaker.leader(proposal.view)
            || !self.notary.verify_block_hash(&proposal.block)
            || !self.notary.verify_qc(&proposal.block.justify)
        {
            return vec![];
        }
        let justify = proposal.block.justify.clone();
        vec![
            Task::Pacemaker(PacemakerInput::ReceivedQC(justify)),
            Task::ProposalOrBuffer { sender, proposal },
        ]
    }

    fn tasks_from_new_vote(&self, signed_vote: SignedVote<S>) -> Vec<Task<S, C>> {
        if !self.pacemaker.is_leader(signed_vote.view() + 1) {
            return vec![];
        }

        if !self.notary.verify_partial(&signed_vote) {
            return vec![];
        }
        vec![Task::Collector(GenericVoteInput(signed_vote))]
    }

    fn tasks_from_new_wish(&self, signed_wish: SignedWish<S>) -> Vec<Task<S, C>> {
        if !self.pacemaker.is_epoch_leader(signed_wish.view()) {
            return vec![];
        }

        if !self.notary.verify_partial(&signed_wish) {
            return vec![];
        }
        vec![Task::Collector(CollectorInput::WishInput(signed_wish))]
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
    ) -> TaskApply<S, C> {
        match task {
            Task::Pacemaker(input) => self.call_pacemaker(storage, input, result),
            Task::Consensus(input) => self.call_consensus(storage, input),
            Task::Collector(input) => Ok(Some(TaskOut::Collector(self.collector.apply(input)))),
            Task::StoreBlock(block) => {
                storage.insert_block(&block)?;
                Ok(None)
            }
            Task::Commit(blocks) => Self::commit_blocks(storage, blocks, result),
            Task::ProposalOrBuffer { sender, proposal } => {
                self.call_proposal_or_buffer(storage, sender, proposal)
            }
            Task::Send(envelope) => {
                result.net.push(envelope);
                Ok(None)
            }
        }
    }

    fn call_pacemaker<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        input: PacemakerInput<S>,
        result: &mut DriverResult<S, C, TP::Clock>,
    ) -> TaskApply<S, C> {
        let old_view = storage.get_cur_view()?;
        let called = self.pacemaker.apply(storage, input)?;
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

    fn call_consensus<DB: ReadWriteStorage<S, C>>(
        &self,
        storage: &mut DB,
        input: ConsensusInput<S, C>,
    ) -> TaskApply<S, C> {
        let cur_view = storage.get_cur_view()?;
        let high_qc = storage.get_high_qc()?;
        Ok(self
            .consensus
            .apply(storage, cur_view, &high_qc, input)?
            .map(TaskOut::Consensus))
    }

    fn call_proposal_or_buffer<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        sender: NodeID,
        proposal: Proposal<S, C>,
    ) -> TaskApply<S, C> {
        if proposal.view > storage.get_cur_view()? {
            self.pending
                .entry(proposal.view)
                .or_default()
                .push((sender, proposal));
            return Ok(None);
        }
        self.call_consensus(storage, ConsensusInput::ReceivedProposal(proposal))
    }

    fn commit_blocks<DB: ReadWriteStorage<S, C>>(
        storage: &mut DB,
        blocks: Vec<Block<S, C>>,
        result: &mut DriverResult<S, C, TP::Clock>,
    ) -> TaskApply<S, C> {
        for block in blocks {
            storage.commit_block(&block)?;
            result.committed.push(block);
        }
        Ok(None)
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
            ConsensusOutput::ConfirmVote { vote, block } => self.on_confirm_vote(vote, block),
            ConsensusOutput::RejectVote(block) => Self::store_and_update(block),
            ConsensusOutput::Commit(blocks) => vec![Task::Commit(blocks)],
            ConsensusOutput::Propose(proposal) => vec![Task::Send(Envelope {
                to: Recipient::All,
                msg: Message::ProposalMsg(proposal),
            })],
        }
    }

    fn on_confirm_vote(&self, vote: Vote<S>, block: Block<S, C>) -> Vec<Task<S, C>> {
        let leader = self.pacemaker.leader(vote.view + 1);
        let mut tasks = Self::store_and_update(block);
        if let Some(signed_vote) = self.notary.sign(vote) {
            tasks.push(Task::Send(Envelope {
                to: Recipient::Node(leader),
                msg: Message::VoteMsg(signed_vote),
            }));
        }
        tasks
    }

    fn store_and_update(block: Block<S, C>) -> Vec<Task<S, C>> {
        vec![
            Task::StoreBlock(block.clone()),
            Task::Consensus(ConsensusInput::Update(block)),
        ]
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
            CollectorOutput::TimeoutOutput(TimeoutCollectorOutput::Ready(tc)) => {
                vec![Task::Pacemaker(PacemakerInput::ReceivedTC(tc))]
            }
            CollectorOutput::TimeoutOutput(
                TimeoutCollectorOutput::AlreadyReady(_) | TimeoutCollectorOutput::NotReady,
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
