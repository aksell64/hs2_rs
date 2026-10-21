use crate::network::message::Envelope;
use crate::protocol::driver::{Driver, DriverInput, DriverOutput};
use crate::protocol::leaders::LeaderPolicy;
use crate::protocol::timeouts::TimeoutPolicy;
use crate::storage::staged::StagedStorage;
use crate::storage::{KVError, Storage, WriteStorage};
use crate::timer::service::TimerCommand;
use crate::types::block::{Block, Command};
use crate::types::clock::{Clock, ClockSource};
use crate::types::crypto::Notary;
use crate::types::genesis::{GenesisConfig, genesis_block};
use crate::types::mempool::Mempool;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::collections::VecDeque;
use std::fmt::{self, Display};

#[derive(Debug)]
pub enum NodeError {
    Storage(KVError),
    InvalidGenesis,
    GenesisMismatch,
}

impl Display for NodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(err) => write!(f, "storage failed: {err}"),
            Self::InvalidGenesis => {
                f.write_str("genesis config does not form a valid genesis block")
            }
            Self::GenesisMismatch => {
                f.write_str("the store belongs to another chain: genesis block is missing")
            }
        }
    }
}

impl std::error::Error for NodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Storage(err) => Some(err),
            Self::InvalidGenesis | Self::GenesisMismatch => None,
        }
    }
}

impl From<KVError> for NodeError {
    fn from(err: KVError) -> Self {
        Self::Storage(err)
    }
}

pub struct Effects<S: Scheme, C: Command, CL: Clock> {
    pub net: Vec<Envelope<S, C>>,
    pub timers: Vec<TimerCommand<CL>>,
    pub sync: Vec<S::Digest>,
    pub committed: Vec<Block<S, C>>,
}

pub struct NodeState<
    S: Scheme,
    C: Command,
    DB: Storage<S, C>,
    M: Mempool<C>,
    TP: TimeoutPolicy<CS::Clock>,
    CS: ClockSource,
    LP: LeaderPolicy,
> {
    driver: Driver<S, C, TP, CS, LP>,
    queue: VecDeque<DriverInput<S, C>>,
    db: DB,
    mempool: M,
}

impl<
    S: Scheme,
    C: Command,
    DB: Storage<S, C>,
    M: Mempool<C>,
    TP: TimeoutPolicy<CS::Clock>,
    CS: ClockSource,
    LP: LeaderPolicy,
> NodeState<S, C, DB, M, TP, CS, LP>
{
    pub fn new(
        notary: Notary<S>,
        mut db: DB,
        mempool: M,
        node_id: NodeID,
        source: CS,
        timeouts: TP,
        leaders: LP,
        genesis: GenesisConfig<S, C>,
    ) -> Result<Self, NodeError> {
        Self::maybe_write_genesis(&notary, &mut db, genesis, &leaders)?;
        let driver = Driver::new(notary, node_id, source, timeouts, leaders);
        Ok(Self {
            driver,
            queue: VecDeque::from([DriverInput::ViewTimerFired(0)]),
            db,
            mempool,
        })
    }

    fn maybe_write_genesis(
        notary: &Notary<S>,
        db: &mut DB,
        genesis: GenesisConfig<S, C>,
        leaders: &LP,
    ) -> Result<(), NodeError> {
        let block = genesis_block(notary, genesis).ok_or(NodeError::InvalidGenesis)?;
        match db.get_high_qc() {
            Ok(_) if db.get_block_by_hash(&block.hash)?.is_some() => Ok(()),
            Ok(_) => Err(NodeError::GenesisMismatch),
            Err(KVError::Missing { .. }) => Ok(Self::write_genesis(db, block, leaders)?),
            Err(e) => Err(e.into()),
        }
    }

    pub fn add_command(&mut self, cmd: C) {
        self.mempool.add_command(cmd);
    }

    pub fn add_event_to_queue(&mut self, evt: DriverInput<S, C>) {
        self.queue.push_back(evt);
    }

    #[cfg(any(test, feature = "testing"))]
    pub fn storage(&self) -> &DB {
        &self.db
    }

    pub fn apply(&mut self) -> Result<Option<Effects<S, C, CS::Clock>>, KVError> {
        let Some(event) = self.queue.pop_front() else {
            return Ok(None);
        };

        if let Some(missing) = self.check_missing_parent(&event)? {
            return Ok(Some(Effects {
                net: Vec::default(),
                timers: Vec::default(),
                sync: vec![missing],
                committed: Vec::default(),
            }));
        }

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
            sync: Vec::default(),
            committed: result.committed,
        }))
    }

    fn check_missing_parent(
        &self,
        input: &DriverInput<S, C>,
    ) -> Result<Option<S::Digest>, KVError> {
        match input {
            DriverInput::NewProposal { proposal, .. } => {
                let parent_hash = proposal.block.parent;
                Ok(self
                    .db
                    .get_block_by_hash(&parent_hash)?
                    .is_none()
                    .then_some(parent_hash))
            }
            _ => Ok(None),
        }
    }

    fn process_committed_blocks(&mut self, committed: &[Block<S, C>]) {
        self.mempool
            .on_commands_commit(committed.iter().map(|block| &block.cmd))
    }

    fn write_genesis(db: &mut DB, genesis: Block<S, C>, leaders: &LP) -> Result<(), KVError> {
        let mut batch = db.new_write_batch();
        batch.insert_block(&genesis)?;
        batch.commit_block(&genesis)?;
        batch.update_high_qc(genesis.justify.clone())?;
        batch.update_locked_qc(genesis.justify)?;
        batch.update_voted_view(0)?;
        batch.update_exec_height(genesis.height)?;
        batch.update_exec_hash(genesis.hash)?;
        batch.update_cur_view(0)?;
        batch.update_cur_leader(leaders.leader(0))?;
        batch.update_proposed(false)?;
        db.commit_write_batch(batch)
    }
}
