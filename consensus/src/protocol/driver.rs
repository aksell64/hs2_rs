use crate::network::message::{Envelope, Message, Recipient};
use crate::protocol::collector::CollectorInput::GenericVoteInput;
use crate::protocol::collector::{
    Collector, CollectorInput, CollectorOutput, GenericCollectorOutput, TimeoutCollectorOutput,
};
use crate::protocol::consensus::{
    Consensus, ConsensusInput, ConsensusOutput, ProposeRequest, extends_justify,
};
use crate::protocol::effects::Effect;
use crate::protocol::leaders::LeaderPolicy;
use crate::protocol::pacemaker::{Pacemaker, PacemakerInput, PacemakerOutput, Status};
use crate::protocol::timeouts::TimeoutPolicy;
use crate::protocol::traits;
use crate::storage::{KVError, ReadStorage, ReadWriteStorage};
use crate::timer::service::TimerCommand;
use crate::types::View;
use crate::types::block::{Block, Command, Proposal};
use crate::types::certificate::{BlockQC, SignedVote, SignedWish, TimeoutQC};
use crate::types::clock::{Clock, ClockSource};
use crate::types::crypto::Notary;
use crate::types::mempool::Mempool;
use crate::types::vote::Vote;
use codec::{Decode, Encode};
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::marker::PhantomData;

#[derive(Encode, Decode)]
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

impl<S: Scheme, C: Command> Clone for DriverInput<S, C> {
    fn clone(&self) -> Self {
        match self {
            Self::NewProposal { sender, proposal } => Self::NewProposal {
                sender: *sender,
                proposal: proposal.clone(),
            },
            Self::NewVote(vote) => Self::NewVote(vote.clone()),
            Self::NewWish(wish) => Self::NewWish(wish.clone()),
            Self::NewStatus(status) => Self::NewStatus(status.clone()),
            Self::NewTC(tc) => Self::NewTC(tc.clone()),
            Self::NewQC(qc) => Self::NewQC(qc.clone()),
            Self::ViewTimerFired(view) => Self::ViewTimerFired(*view),
            Self::LockTimerFired(view) => Self::LockTimerFired(*view),
            Self::BlockSynced(block) => Self::BlockSynced(block.clone()),
        }
    }
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
    CO: traits::Consensus<S>,
    PM: traits::Pacemaker<S>,
    CL: traits::Collector<S>,
> {
    notary: Notary<S>,
    consensus: CO,
    pacemaker: PM,
    collector: CL,
    _command: PhantomData<C>,
}

impl<S: Scheme, C: Command, TP: TimeoutPolicy<CS::Clock>, CS: ClockSource, LP: LeaderPolicy>
    Driver<S, C, Consensus<S>, Pacemaker<S, TP, CS, LP>, Collector<S>>
{
    pub fn new(notary: Notary<S>, id: NodeID, source: CS, policy: TP, leaders: LP) -> Self {
        Self::with_components(
            notary.clone(),
            Consensus::new(notary.clone()),
            Pacemaker::new(notary.clone(), id, source, policy, leaders),
            Collector::new(notary),
        )
    }
}

impl<
    S: Scheme,
    C: Command,
    CO: traits::Consensus<S>,
    PM: traits::Pacemaker<S>,
    CL: traits::Collector<S>,
> Driver<S, C, CO, PM, CL>
{
    pub fn with_components(notary: Notary<S>, consensus: CO, pacemaker: PM, collector: CL) -> Self {
        Self {
            notary,
            consensus,
            pacemaker,
            collector,
            _command: PhantomData,
        }
    }

    pub fn apply<DB: ReadWriteStorage<S, C>, M: Mempool<C>>(
        &mut self,
        storage: &mut DB,
        mempool: &mut M,
        evt: DriverInput<S, C>,
    ) -> Result<DriverResult<S, C, PM::Clock>, KVError> {
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
                stack.extend(self.process_out(storage, mempool, out, &mut result)?);
            }
        }
        Ok(result)
    }

    fn prepare_event<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        event: DriverInput<S, C>,
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        let tasks = match event {
            DriverInput::NewProposal { sender, proposal } => {
                self.tasks_from_new_proposal(storage, sender, proposal, result)?
            }
            DriverInput::NewVote(signed_vote) => self.tasks_from_new_vote(storage, signed_vote)?,
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
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> TaskApply<S, C> {
        match task {
            Task::Pacemaker(input) => self.call_pacemaker(storage, input, result),
            Task::Consensus(input) => self.call_consensus(storage, input),
            Task::Collector(input) => Ok(Some(TaskOut::Collector(
                self.collector.apply(storage, input)?,
            ))),
            Task::StoreBlock(block) => {
                storage.insert_block(&block)?;
                Ok(None)
            }
            Task::Commit(blocks) => Self::commit_blocks(storage, blocks, result),
            Task::Send(envelope) => {
                result.net.push(envelope);
                Ok(None)
            }
        }
    }

    fn process_out<DB: ReadWriteStorage<S, C>, M: Mempool<C>>(
        &mut self,
        storage: &mut DB,
        mempool: &mut M,
        output: TaskOut<S, C>,
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        let tasks = match output {
            TaskOut::Pacemaker(PacemakerOutput::ProposeNow(view)) => {
                let request = Self::propose_request(mempool, view);
                result.replay.extend(storage.take_inputs_up_to_view(view)?);
                TasksStack::from_vec(vec![Task::Consensus(ConsensusInput::MakeProposal(request))])
            }
            TaskOut::Pacemaker(PacemakerOutput::ViewAdvanced(view)) => {
                result.replay.extend(storage.take_inputs_up_to_view(view)?);
                TasksStack::empty()
            }
            TaskOut::Pacemaker(PacemakerOutput::NoPacemakerOutput) => TasksStack::empty(),
            TaskOut::Consensus(ConsensusOutput::ConfirmVote { vote, block }) => {
                self.on_confirm_vote(storage, vote, block, result)?
            }
            TaskOut::Consensus(ConsensusOutput::RejectVote(block)) => {
                let hash = block.hash;
                result.replay.extend(storage.take_inputs_for_block(hash)?);
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
        };
        Ok(tasks)
    }

    fn tasks_from_synced_block<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        block: Block<S, C>,
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        if !self.notary.verify_block_hash(&block)
            || !self.notary.verify_qc(&block.justify)
            || !extends_justify(storage, &block)?
        {
            return Ok(TasksStack::empty());
        }

        let justify = block.justify.clone();
        result
            .replay
            .extend(storage.take_inputs_for_block(block.hash)?);
        Ok(TasksStack::from_vec(vec![
            Task::Pacemaker(PacemakerInput::ReceivedQC(justify)),
            Task::Consensus(ConsensusInput::Update(block.clone())),
            Task::StoreBlock(block),
        ]))
    }

    fn tasks_from_new_proposal<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        sender: NodeID,
        proposal: Proposal<S, C>,
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        if sender != self.pacemaker.leader(proposal.view)
            || !self.notary.verify_block_hash(&proposal.block)
            || !self.notary.verify_qc(&proposal.block.justify)
            || proposal.block.parent != proposal.block.justify.block()
            || proposal.block.justify.view() > proposal.view
        {
            return Ok(TasksStack::empty());
        }
        if let Some(missing) = Self::check_missing_parent(storage, proposal.block.parent)? {
            storage.buffer_for_block(missing, DriverInput::NewProposal { sender, proposal })?;
            result.sync.push(missing);
            return Ok(TasksStack::empty());
        }

        let justify = proposal.block.justify.clone();

        if proposal.view == proposal.block.justify.view() + 1 {
            return Ok(TasksStack::from_vec(vec![
                Task::Consensus(ConsensusInput::ReceivedProposal(proposal)),
                Task::Pacemaker(PacemakerInput::ReceivedQC(justify)),
            ]));
        }

        if proposal.view > storage.get_cur_view()? {
            storage
                .buffer_for_view(proposal.view, DriverInput::NewProposal { sender, proposal })?;

            return Ok(TasksStack::from_vec(vec![Task::Pacemaker(
                PacemakerInput::ReceivedQC(justify),
            )]));
        }

        Ok(TasksStack::from_vec(vec![
            Task::Consensus(ConsensusInput::ReceivedProposal(proposal)),
            Task::Pacemaker(PacemakerInput::ReceivedQC(justify)),
        ]))
    }

    fn tasks_from_new_status<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        status: Status<S>,
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        if !self.pacemaker.is_leader(status.view) || !self.notary.verify_qc(&status.qc) {
            return Ok(TasksStack::empty());
        }
        if let Some(missing) = Self::check_missing_parent(storage, status.qc.block())? {
            storage.buffer_for_block(missing, DriverInput::NewStatus(status))?;
            result.sync.push(missing);
            return Ok(TasksStack::empty());
        }

        if status.view > storage.get_cur_view()? {
            let status_qc = status.qc.clone();
            storage.buffer_for_view(status.view, DriverInput::NewStatus(status))?;
            return Ok(TasksStack::from_vec(vec![Task::Pacemaker(
                PacemakerInput::ReceivedQC(status_qc),
            )]));
        }

        Ok(TasksStack::from_vec(vec![Task::Pacemaker(
            PacemakerInput::ReceivedStatus(status),
        )]))
    }

    fn tasks_from_new_qc<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        qc: BlockQC<S>,
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
        if !self.notary.verify_qc(&qc) {
            return Ok(TasksStack::empty());
        }
        if let Some(missing) = Self::check_missing_parent(storage, qc.block())? {
            storage.buffer_for_block(missing, DriverInput::NewQC(qc))?;
            result.sync.push(missing);
            return Ok(TasksStack::empty());
        }
        Ok(TasksStack::from_vec(vec![Task::Pacemaker(
            PacemakerInput::ReceivedQC(qc),
        )]))
    }

    fn tasks_from_new_vote<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        signed_vote: SignedVote<S>,
    ) -> Result<TasksStack<S, C>, KVError> {
        if !self.pacemaker.is_leader(signed_vote.view() + 1) {
            return Ok(TasksStack::empty());
        }

        if !self.notary.verify_partial(&signed_vote) {
            return Ok(TasksStack::empty());
        }

        if signed_vote.view() > storage.get_cur_view()? {
            storage.buffer_for_view(signed_vote.view(), DriverInput::NewVote(signed_vote))?;
            return Ok(TasksStack::empty());
        }

        Ok(TasksStack::from_vec(vec![Task::Collector(
            GenericVoteInput(signed_vote),
        )]))
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
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> TaskApply<S, C> {
        let called = self.pacemaker.apply(storage, input)?;
        for effect in called.effects {
            match effect {
                Effect::Send(envelope) => result.net.push(envelope),
                Effect::Timer(command) => result.timers.push(command),
            }
        }
        match called.out {
            PacemakerOutput::NoPacemakerOutput => Ok(None),
            out => Ok(Some(TaskOut::Pacemaker(out))),
        }
    }

    fn call_consensus<DB: ReadWriteStorage<S, C>>(
        &mut self,
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

    fn check_missing_parent<DB: ReadStorage<S, C>>(
        storage: &DB,
        block_hash: S::Digest,
    ) -> Result<Option<S::Digest>, KVError> {
        Ok(storage
            .get_block_by_hash(&block_hash)?
            .is_none()
            .then_some(block_hash))
    }

    fn commit_blocks<DB: ReadWriteStorage<S, C>>(
        storage: &mut DB,
        blocks: Vec<Block<S, C>>,
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> TaskApply<S, C> {
        for block in blocks {
            storage.commit_block(&block)?;
            result.committed.push(block);
        }
        Ok(None)
    }

    fn on_confirm_vote<DB: ReadWriteStorage<S, C>>(
        &mut self,
        storage: &mut DB,
        vote: Vote<S>,
        block: Block<S, C>,
        result: &mut DriverResult<S, C, PM::Clock>,
    ) -> Result<TasksStack<S, C>, KVError> {
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
        result.replay.extend(storage.take_inputs_for_block(hash)?);
        Ok(TasksStack::from_vec(tasks))
    }

    fn propose_request<M: Mempool<C>>(mempool: &mut M, view: View) -> ProposeRequest<C> {
        ProposeRequest {
            cmd: mempool.next_command(),
            view,
        }
    }
}
