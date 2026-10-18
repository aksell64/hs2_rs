use crate::storage::{KVError, ReadStorage, Storage, WriteStorage};
use crate::types::View;
use crate::types::block::{Block, Command};
use crate::types::certificate::BlockQC;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::collections::HashMap;

pub struct StagedStorage<'a, S: Scheme, C: Command, DB: Storage<S, C>> {
    storage: &'a DB,
    batch: DB::WriteBatch,
    blocks: HashMap<S::Digest, Block<S, C>>,
    high_qc: Option<BlockQC<S>>,
    locked_qc: Option<BlockQC<S>>,
    voted_view: Option<View>,
    exec_height: Option<u64>,
    exec_hash: Option<S::Digest>,
    cur_view: Option<View>,
    cur_leader: Option<NodeID>,
    proposed: Option<bool>,
}

impl<'a, S: Scheme, C: Command, DB: Storage<S, C>> StagedStorage<'a, S, C, DB> {
    pub fn new(storage: &'a DB) -> Self {
        Self {
            storage,
            batch: storage.new_write_batch(),
            blocks: HashMap::new(),
            high_qc: None,
            locked_qc: None,
            voted_view: None,
            exec_height: None,
            exec_hash: None,
            cur_view: None,
            cur_leader: None,
            proposed: None,
        }
    }

    pub fn into_batch(self) -> DB::WriteBatch {
        self.batch
    }
}

impl<S: Scheme, C: Command, DB: Storage<S, C>> ReadStorage<S, C> for StagedStorage<'_, S, C, DB> {
    fn get_block_by_hash(&self, hash: &S::Digest) -> Result<Option<Block<S, C>>, KVError> {
        match self.blocks.get(hash) {
            Some(block) => Ok(Some(block.clone())),
            None => self.storage.get_block_by_hash(hash),
        }
    }

    fn get_cert_by_view(&self, view: View) -> Result<Option<BlockQC<S>>, KVError> {
        self.storage.get_cert_by_view(view)
    }

    fn get_high_qc(&self) -> Result<BlockQC<S>, KVError> {
        match &self.high_qc {
            Some(qc) => Ok(qc.clone()),
            None => self.storage.get_high_qc(),
        }
    }

    fn get_locked_qc(&self) -> Result<BlockQC<S>, KVError> {
        match &self.locked_qc {
            Some(qc) => Ok(qc.clone()),
            None => self.storage.get_locked_qc(),
        }
    }

    fn get_voted_view(&self) -> Result<View, KVError> {
        match self.voted_view {
            Some(view) => Ok(view),
            None => self.storage.get_voted_view(),
        }
    }

    fn get_exec_height(&self) -> Result<u64, KVError> {
        match self.exec_height {
            Some(height) => Ok(height),
            None => self.storage.get_exec_height(),
        }
    }

    fn get_exec_hash(&self) -> Result<S::Digest, KVError> {
        match self.exec_hash {
            Some(hash) => Ok(hash),
            None => self.storage.get_exec_hash(),
        }
    }

    fn get_cur_view(&self) -> Result<View, KVError> {
        match self.cur_view {
            Some(view) => Ok(view),
            None => self.storage.get_cur_view(),
        }
    }

    fn get_cur_leader(&self) -> Result<NodeID, KVError> {
        match self.cur_leader {
            Some(leader) => Ok(leader),
            None => self.storage.get_cur_leader(),
        }
    }

    fn get_proposed(&self) -> Result<bool, KVError> {
        match self.proposed {
            Some(proposed) => Ok(proposed),
            None => self.storage.get_proposed(),
        }
    }
}

impl<S: Scheme, C: Command, DB: Storage<S, C>> WriteStorage<S, C> for StagedStorage<'_, S, C, DB> {
    fn insert_block(&mut self, block: &Block<S, C>) -> Result<(), KVError> {
        // todo:  cloning the full block, its slow
        self.blocks.insert(block.hash, block.clone());
        self.batch.insert_block(block)
    }

    fn commit_block(&mut self, block: &Block<S, C>) -> Result<(), KVError> {
        self.batch.commit_block(block)
    }

    fn update_high_qc(&mut self, qc: BlockQC<S>) -> Result<(), KVError> {
        self.high_qc = Some(qc.clone());
        self.batch.update_high_qc(qc)
    }

    fn update_locked_qc(&mut self, qc: BlockQC<S>) -> Result<(), KVError> {
        self.locked_qc = Some(qc.clone());
        self.batch.update_locked_qc(qc)
    }

    fn update_voted_view(&mut self, view: View) -> Result<(), KVError> {
        self.voted_view = Some(view);
        self.batch.update_voted_view(view)
    }

    fn update_exec_height(&mut self, height: u64) -> Result<(), KVError> {
        self.exec_height = Some(height);
        self.batch.update_exec_height(height)
    }

    fn update_exec_hash(&mut self, hash: S::Digest) -> Result<(), KVError> {
        self.exec_hash = Some(hash);
        self.batch.update_exec_hash(hash)
    }

    fn update_cur_view(&mut self, view: View) -> Result<(), KVError> {
        self.cur_view = Some(view);
        self.batch.update_cur_view(view)
    }

    fn update_cur_leader(&mut self, leader: NodeID) -> Result<(), KVError> {
        self.cur_leader = Some(leader);
        self.batch.update_cur_leader(leader)
    }

    fn update_proposed(&mut self, proposed: bool) -> Result<(), KVError> {
        self.proposed = Some(proposed);
        self.batch.update_proposed(proposed)
    }
}
