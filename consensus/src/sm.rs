use crate::network::message::Envelope;
use crate::protocol::driver::{Driver, DriverEvent, DriverOutput};
use crate::storage::kv::{KVError, Storage};
use crate::storage::staged::StagedStorage;
use crate::timer::service::TimerCommand;
use crate::types::block::{Block, Command};
use crate::types::mempool::Mempool;
use hotstuff_crypto::sign::Scheme;
use std::collections::VecDeque;

pub struct Effects<S: Scheme, C: Command> {
    pub net: Vec<Envelope<S, C>>,
    pub timers: Vec<TimerCommand>,
}

pub struct NodeState<S: Scheme, C: Command, DB: Storage<S, C>, M: Mempool<C>> {
    driver: Driver<S, C>,
    queue: VecDeque<DriverEvent<S, C>>,
    db: DB,
    mempool: M,
}

impl<S: Scheme, C: Command, DB: Storage<S, C>, M: Mempool<C>> NodeState<S, C, DB, M> {
    pub fn add_event_to_queue(&mut self, evt: DriverEvent<S, C>) {
        self.queue.push_back(evt);
    }

    pub fn apply(&mut self) -> Result<Option<Effects<S, C>>, KVError> {
        let Some(event) = self.queue.pop_front() else {
            return Ok(None);
        };
        let mut staged = StagedStorage::new(&self.db);
        let result = self.driver.apply(&mut staged, event)?;
        let batch = staged.into_batch();
        self.db.commit_write_batch(batch)?;

        self.process_committed_blocks(&result.committed);

        let mut effects = Effects {
            net: result.net,
            timers: Vec::new(),
        };
        for out in result.out {
            match out {
                DriverOutput::NewView(view) => self.queue.push_front(DriverEvent::EnterView(view)),
                DriverOutput::Replay(replay) => self.queue.push_front(replay),
                DriverOutput::SetViewTimer(view) => effects.timers.push(TimerCommand::StartView(view)),
                DriverOutput::SetLockTimer(view) => effects.timers.push(TimerCommand::StartLock(view)),
                DriverOutput::ClearLockTimer(view) => effects.timers.push(TimerCommand::StopLock(view)),
            }
        }
        Ok(Some(effects))
    }

    fn process_committed_blocks(&mut self, committed: &[Block<S, C>]) {
        self.mempool.on_commands_commit(committed.iter().map(|block| &block.cmd))
    }
}
