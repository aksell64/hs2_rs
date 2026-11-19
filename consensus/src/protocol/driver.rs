use crate::network::message::{Envelope, Message, Recipient};
use crate::protocol::collector::CollectorInput::GenericVoteInput;
use crate::protocol::collector::{
    Collector, CollectorInput, CollectorOutput, GenericCollectorOutput, TimeoutCollectorOutput,
};
use crate::protocol::consensus::{Consensus, ConsensusInput, ConsensusOutput, ProposeRequest};
use crate::protocol::effects::Effect;
use crate::protocol::leaders::LeaderPolicy;
use crate::protocol::orphans::Orphans;
use crate::protocol::pacemaker::{Pacemaker, PacemakerInput, PacemakerOutput, Status};
use crate::protocol::timeouts::TimeoutPolicy;
use crate::storage::{KVError, ReadStorage, ReadWriteStorage};
use crate::timer::service::TimerCommand;
use crate::types::View;
use crate::types::block::{Block, Command, Proposal};
use crate::types::certificate::{BlockQC, SignedVote, SignedWish, TimeoutQC};
use crate::types::clock::{Clock, ClockSource};
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
    NewQC(BlockQC<S>),
    ViewTimerFired(View),
    LockTimerFired(View),
    BlockSynced(Block<S, C>),
}

pub struct DriverResult<S: Scheme, C: Command, CL: Clock> {
    pub net: Vec<Envelope<S, C>>,
    pub timers: Vec<TimerCommand<CL>>,
    pub replay: Vec<DriverInput<S, C>>,
    pub sync: Vec<S::Digest>,
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
    fn empty() -> Self {
        Self(Vec::new())
    }

    fn from_vec(tasks: Vec<Task<S, C>>) -> Self {
        Self(tasks)
    }

    fn extend(&mut self, tasks: Self) {
        self.0.extend(tasks.0);
    }

    fn pop(&mut self) -> Option<Task<S, C>> {
        self.0.pop()
    }
}

pub struct Driver<
    S: Scheme,
    C: Command,
    TP: TimeoutPolicy<CS::Clock>,
    CS: ClockSource,
    LP: LeaderPolicy,
> {
    notary: Notary<S>,
    consensus: Consensus<S>,
    pacemaker: Pacemaker<S, TP, CS, LP>,
    collector: Collector<S>,
    pending: BTreeMap<View, Vec<(NodeID, Proposal<S, C>)>>,
    orphans: Orphans<S, C>,
}

impl<S: Scheme, C: Command, TP: TimeoutPolicy<CS::Clock>, CS: ClockSource, LP: LeaderPolicy>
    Driver<S, C, TP, CS, LP>
{
    pub fn new(notary: Notary<S>, id: NodeID, source: CS, policy: TP, leaders: LP) -> Self {
        Self {
            consensus: Consensus::new(notary.clone()),
            pacemaker: Pacemaker::new(notary.clone(), id, source, policy, leaders),
            collector: Collector::new(notary.clone()),
            notary,
            pending: BTreeMap::new(),
            orphans: Orphans::new(),
        }
    }

    pub fn apply<DB: ReadWriteStorage<S, C>, M: Mempool<C>>(
        &mut self,
        storage: &mut DB,
        mempool: &mut M,
        evt: DriverInput<S, C>,
    ) -> Result<DriverResult<S, C, CS::Clock>, KVError> {
        let mut result = DriverResult {
            net: Vec::new(),
            timers: Vec::new(),
            replay: Vec::new(),
            sync: Vec::new(),
            committed: Vec::new(),
        };
        let mut stack = self.prepare_event(storage, evt, &mut result)?;
        while let Some(task) = stack.pop() {
            if let Some(out) = self.process_input(storage, task, &mut result)? {
                stack.extend(self.process_out(mempool, out, &mut result));
            }
        }
        Ok(result)
    }

    fn prepare_event<DB: ReadStorage<S, C>>(
        &mut self,
        storage: &DB,
        event: DriverInput<S, C>,
        result: &mut DriverResult<S, C, CS::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        let tasks = match event {
            DriverInput::NewProposal { sender, proposal } => {
                self.tasks_from_new_proposal(storage, sender, proposal, result)?
            }
            DriverInput::NewVote(signed_vote) => self.tasks_from_new_vote(signed_vote),
            DriverInput::NewWish(signed_wish) => self.tasks_from_new_wish(signed_wish),
            DriverInput::NewStatus(status) => {
                self.tasks_from_new_status(storage, status, result)?
            }
            DriverInput::NewTC(tc) => {
                TasksStack::from_vec(vec![Task::Pacemaker(PacemakerInput::ReceivedTC(tc))])
            }
            DriverInput::NewQC(qc) => self.tasks_from_new_qc(storage, qc, result)?,
            DriverInput::ViewTimerFired(view) => TasksStack::from_vec(vec![Task::Pacemaker(
                PacemakerInput::ViewTimerExpired(view),
            )]),
            DriverInput::LockTimerFired(view) => TasksStack::from_vec(vec![Task::Pacemaker(
                PacemakerInput::LockTimerExpired(view),
            )]),
            DriverInput::BlockSynced(block) => {
                self.tasks_from_synced_block(storage, block, result)?
            }
        };
        Ok(tasks)
    }

    fn process_input<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        task: Task<S, C>,
        result: &mut DriverResult<S, C, CS::Clock>,
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

    fn process_out<M: Mempool<C>>(
        &mut self,
        mempool: &mut M,
        output: TaskOut<S, C>,
        result: &mut DriverResult<S, C, CS::Clock>,
    ) -> TasksStack<S, C> {
        match output {
            TaskOut::Pacemaker(PacemakerOutput::ProposeNow(view)) => {
                let request = Self::propose_request(mempool, view);
                TasksStack::from_vec(vec![Task::Consensus(ConsensusInput::MakeProposal(request))])
            }
            TaskOut::Pacemaker(PacemakerOutput::NoPacemakerOutput) => TasksStack::empty(),
            TaskOut::Consensus(ConsensusOutput::ConfirmVote { vote, block }) => {
                self.on_confirm_vote(vote, block, result)
            }
            TaskOut::Consensus(ConsensusOutput::RejectVote(block)) => {
                let hash = block.hash;
                result.replay.extend(self.orphans.take_inputs(&hash));
                TasksStack::from_vec(vec![
                    Task::Consensus(ConsensusInput::Update(block.clone())),
                    Task::StoreBlock(block),
                ])
            }
            TaskOut::Consensus(ConsensusOutput::Commit(blocks)) => {
                TasksStack::from_vec(vec![Task::Commit(blocks)])
            }
            TaskOut::Consensus(ConsensusOutput::Propose(proposal)) => {
                TasksStack::from_vec(vec![Task::Send(Envelope {
                    to: Recipient::All,
                    msg: Message::ProposalMsg(proposal),
                })])
            }
            TaskOut::Collector(CollectorOutput::GenericOutput(GenericCollectorOutput::Ready(
                qc,
            ))) => {
                result.replay.push(DriverInput::NewQC(qc));
                TasksStack::empty()
            }
            TaskOut::Collector(CollectorOutput::GenericOutput(
                GenericCollectorOutput::AlreadyReady(_) | GenericCollectorOutput::NotReady,
            )) => TasksStack::empty(),
            TaskOut::Collector(CollectorOutput::TimeoutOutput(TimeoutCollectorOutput::Ready(
                tc,
            ))) => TasksStack::from_vec(vec![Task::Pacemaker(PacemakerInput::ReceivedTC(tc))]),
            TaskOut::Collector(CollectorOutput::TimeoutOutput(
                TimeoutCollectorOutput::AlreadyReady(_) | TimeoutCollectorOutput::NotReady,
            )) => TasksStack::empty(),
        }
    }

    fn tasks_from_synced_block<DB: ReadStorage<S, C>>(
        &mut self,
        storage: &DB,
        block: Block<S, C>,
        result: &mut DriverResult<S, C, CS::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        if !self.notary.verify_block_hash(&block)
            || !self.notary.verify_qc(&block.justify)
            || !Consensus::extends_justify(storage, &block)?
        {
            return Ok(TasksStack::empty());
        }

        let justify = block.justify.clone();
        let block_hash = block.hash;
        result.replay.extend(self.orphans.take_inputs(&block_hash));
        Ok(TasksStack::from_vec(vec![
            Task::Pacemaker(PacemakerInput::ReceivedQC(justify)),
            Task::Consensus(ConsensusInput::Update(block.clone())),
            Task::StoreBlock(block),
        ]))
    }

    fn tasks_from_new_proposal<DB: ReadStorage<S, C>>(
        &mut self,
        storage: &DB,
        sender: NodeID,
        proposal: Proposal<S, C>,
        result: &mut DriverResult<S, C, CS::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        if sender != self.pacemaker.leader(proposal.view)
            || !self.notary.verify_block_hash(&proposal.block)
            || !self.notary.verify_qc(&proposal.block.justify)
            || proposal.block.parent != proposal.block.justify.block()
        {
            return Ok(TasksStack::empty());
        }
        if let Some(missing) = Self::check_missing_parent(storage, &proposal.block)? {
            self.orphans
                .buffer_input(missing, DriverInput::NewProposal { sender, proposal });
            result.sync.push(missing);
            return Ok(TasksStack::empty());
        }
        let justify = proposal.block.justify.clone();
        Ok(TasksStack::from_vec(vec![
            Task::ProposalOrBuffer { sender, proposal },
            Task::Pacemaker(PacemakerInput::ReceivedQC(justify)),
        ]))
    }

    fn tasks_from_new_status<DB: ReadStorage<S, C>>(
        &mut self,
        storage: &DB,
        status: Status<S>,
        result: &mut DriverResult<S, C, CS::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        if !self.pacemaker.is_leader(status.view) || !self.notary.verify_qc(&status.qc) {
            return Ok(TasksStack::empty());
        }
        let hash = status.qc.block();
        if storage.get_block_by_hash(&hash)?.is_none() {
            self.orphans
                .buffer_input(hash, DriverInput::NewStatus(status));
            result.sync.push(hash);
            return Ok(TasksStack::empty());
        }
        Ok(TasksStack::from_vec(vec![Task::Pacemaker(
            PacemakerInput::ReceivedStatus(status),
        )]))
    }

    fn tasks_from_new_qc<DB: ReadStorage<S, C>>(
        &mut self,
        storage: &DB,
        qc: BlockQC<S>,
        result: &mut DriverResult<S, C, CS::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        if !self.notary.verify_qc(&qc) {
            return Ok(TasksStack::empty());
        }
        let hash = qc.block();
        if storage.get_block_by_hash(&hash)?.is_none() {
            self.orphans.buffer_input(hash, DriverInput::NewQC(qc));
            result.sync.push(hash);
            return Ok(TasksStack::empty());
        }
        Ok(TasksStack::from_vec(vec![Task::Pacemaker(
            PacemakerInput::ReceivedQC(qc),
        )]))
    }

    fn tasks_from_new_vote(&self, signed_vote: SignedVote<S>) -> TasksStack<S, C> {
        if !self.pacemaker.is_leader(signed_vote.view() + 1) {
            return TasksStack::empty();
        }

        if !self.notary.verify_partial(&signed_vote) {
            return TasksStack::empty();
        }
        TasksStack::from_vec(vec![Task::Collector(GenericVoteInput(signed_vote))])
    }

    fn tasks_from_new_wish(&self, signed_wish: SignedWish<S>) -> TasksStack<S, C> {
        if !self.pacemaker.is_epoch_leader(signed_wish.view()) {
            return TasksStack::empty();
        }

        if !self.notary.verify_partial(&signed_wish) {
            return TasksStack::empty();
        }
        TasksStack::from_vec(vec![Task::Collector(CollectorInput::WishInput(
            signed_wish,
        ))])
    }

    fn call_pacemaker<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        input: PacemakerInput<S>,
        result: &mut DriverResult<S, C, CS::Clock>,
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

    fn check_missing_parent<DB: ReadStorage<S, C>>(
        storage: &DB,
        block: &Block<S, C>,
    ) -> Result<Option<S::Digest>, KVError> {
        let parent_hash = block.parent;
        Ok(storage
            .get_block_by_hash(&parent_hash)?
            .is_none()
            .then_some(parent_hash))
    }

    fn commit_blocks<DB: ReadWriteStorage<S, C>>(
        storage: &mut DB,
        blocks: Vec<Block<S, C>>,
        result: &mut DriverResult<S, C, CS::Clock>,
    ) -> TaskApply<S, C> {
        for block in blocks {
            storage.commit_block(&block)?;
            result.committed.push(block);
        }
        Ok(None)
    }

    fn release_pending(&mut self, view: View, result: &mut DriverResult<S, C, CS::Clock>) {
        let released = self.pending.remove(&view).unwrap_or_default();
        result.replay.extend(
            released
                .into_iter()
                .map(|(sender, proposal)| DriverInput::NewProposal { sender, proposal }),
        );
    }

    fn on_confirm_vote(
        &mut self,
        vote: Vote<S>,
        block: Block<S, C>,
        result: &mut DriverResult<S, C, CS::Clock>,
    ) -> TasksStack<S, C> {
        let leader = self.pacemaker.leader(vote.view + 1);
        let hash = block.hash;
        let mut tasks = Vec::new();
        if let Some(signed_vote) = self.notary.sign(vote) {
            tasks.push(Task::Send(Envelope {
                to: Recipient::Node(leader),
                msg: Message::VoteMsg(signed_vote),
            }));
        }
        tasks.extend([
            Task::Consensus(ConsensusInput::Update(block.clone())),
            Task::StoreBlock(block),
        ]);
        result.replay.extend(self.orphans.take_inputs(&hash));
        TasksStack::from_vec(tasks)
    }

    fn propose_request<M: Mempool<C>>(mempool: &mut M, view: View) -> ProposeRequest<C> {
        ProposeRequest {
            cmd: mempool.next_command(),
            view,
        }
    }
}
