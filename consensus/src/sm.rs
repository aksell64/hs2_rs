use crate::network::message::Envelope;
use crate::protocol::driver::{Driver, DriverInput, DriverOutput};
use crate::protocol::leaders::LeaderPolicy;
use crate::protocol::timeouts::TimeoutPolicy;
use crate::storage::{KVError, Storage};
use crate::storage::staged::StagedStorage;
use crate::timer::service::TimerCommand;
use crate::types::block::{Block, Command};
use crate::types::clock::{Clock, Source};
use crate::types::mempool::Mempool;
use hotstuff_crypto::sign::Scheme;
use std::collections::VecDeque;

pub struct Effects<S: Scheme, C: Command, CL: Clock> {
    pub net: Vec<Envelope<S, C>>,
    pub timers: Vec<TimerCommand<CL>>,
}

pub struct NodeState<
    S: Scheme,
    C: Command,
    DB: Storage<S, C>,
    M: Mempool<C>,
    TP: TimeoutPolicy,
    TS: Source<Clock = TP::Clock>,
    LP: LeaderPolicy,
> {
    driver: Driver<S, C, TP, TS, LP>,
    queue: VecDeque<DriverInput<S, C>>,
    db: DB,
    mempool: M,
}

impl<
    S: Scheme,
    C: Command,
    DB: Storage<S, C>,
    M: Mempool<C>,
    TP: TimeoutPolicy,
    TS: Source<Clock = TP::Clock>,
    LP: LeaderPolicy,
> NodeState<S, C, DB, M, TP, TS, LP>
{
    pub fn add_event_to_queue(&mut self, evt: DriverInput<S, C>) {
        self.queue.push_back(evt);
    }

    pub fn apply(&mut self) -> Result<Option<Effects<S, C, TP::Clock>>, KVError> {
        let Some(event) = self.queue.pop_front() else {
            return Ok(None);
        };
        let mut staged = StagedStorage::new(&self.db);
        let result = self.driver.apply(&mut staged, &mut self.mempool, event)?;
        let batch = staged.into_batch();
        self.db.commit_write_batch(batch)?;

        self.process_committed_blocks(&result.committed);

        for out in result.out {
            match out {
                DriverOutput::Replay(replay) => self.queue.push_front(replay),
            }
        }

        Ok(Some(Effects {
            net: result.net,
            timers: result.timers,
        }))
    }

    fn process_committed_blocks(&mut self, committed: &[Block<S, C>]) {
        self.mempool.on_commands_commit(committed.iter().map(|block| &block.cmd))
    }
}
