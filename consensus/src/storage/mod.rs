pub mod kv;
pub mod kv_storage;
pub mod staged;

pub use kv::KVError;

use crate::types::View;
use crate::types::block::{Block, Command};
use crate::types::certificate::{BlockQC, SignedVote, SignedWish, TimeoutQC};
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;

pub trait ReadStorage<S: Scheme, C: Command> {
    fn get_block_by_hash(&self, hash: &S::Digest) -> Result<Option<Block<S, C>>, KVError>;
    fn get_block_qc(&self, view: View) -> Result<Option<BlockQC<S>>, KVError>;
    fn get_timeout_qc(&self, view: View) -> Result<Option<TimeoutQC<S>>, KVError>;
    fn exists_vote(&self, view: View, sender: NodeID) -> Result<bool, KVError>;
    fn get_votes(&self, view: View) -> Result<Vec<SignedVote<S>>, KVError>;
    fn get_votes_count(&self, view: View, block: &S::Digest) -> Result<usize, KVError>;
    fn exists_wish_vote(&self, view: View, sender: NodeID) -> Result<bool, KVError>;
    fn get_wish_votes(&self, view: View) -> Result<Vec<SignedWish<S>>, KVError>;
    fn get_wish_votes_count(&self, view: View) -> Result<usize, KVError>;
    fn get_high_qc(&self) -> Result<BlockQC<S>, KVError>;
    fn get_locked_qc(&self) -> Result<BlockQC<S>, KVError>;
    fn get_voted_view(&self) -> Result<View, KVError>;
    fn get_exec_height(&self) -> Result<u64, KVError>;
    fn get_exec_hash(&self) -> Result<S::Digest, KVError>;
    fn get_cur_view(&self) -> Result<View, KVError>;
    fn get_cur_leader(&self) -> Result<NodeID, KVError>;
    fn get_proposed_view(&self) -> Result<View, KVError>;
    fn get_scheduled_epoch(&self) -> Result<Option<View>, KVError>;
    fn get_lock_expired(&self) -> Result<View, KVError>;
    fn get_statuses_count(&self, view: View) -> Result<usize, KVError>;
    fn exists_wish(&self, view: View) -> Result<bool, KVError>;
    fn exists_relayed_tc(&self, view: View) -> Result<bool, KVError>;
    fn get_block_branch(
        &self,
        block: Block<S, C>,
        depth: usize,
    ) -> Result<Vec<Block<S, C>>, KVError> {
        let mut branch = Vec::with_capacity(depth);
        let mut current = Some(block);
        while branch.len() < depth {
            let Some(block) = current else { break };
            current = self.get_block_by_hash(&block.parent)?;
            branch.push(block);
        }
        Ok(branch)
    }
}

pub trait Storage<S: Scheme, C: Command>: ReadStorage<S, C> + Send + 'static {
    type WriteBatch: WriteStorage<S, C>;

    fn new_write_batch(&self) -> Self::WriteBatch;
    fn commit_write_batch(&mut self, batch: Self::WriteBatch) -> Result<(), KVError>;
    fn has_status(&self, node_id: NodeID, view: View) -> Result<bool, KVError>;
    fn get_vote_block(&self, view: View, sender: NodeID) -> Result<Option<S::Digest>, KVError>;
}

pub trait WriteStorage<S: Scheme, C: Command> {
    fn insert_block(&mut self, block: &Block<S, C>) -> Result<(), KVError>;
    fn commit_block(&mut self, block: &Block<S, C>) -> Result<(), KVError>;
    fn insert_block_qc(&mut self, qc: &BlockQC<S>) -> Result<(), KVError>;
    fn insert_timeout_qc(&mut self, qc: &TimeoutQC<S>) -> Result<(), KVError>;
    fn insert_vote(&mut self, vote: &SignedVote<S>) -> Result<(), KVError>;
    fn insert_wish_vote(&mut self, wish: &SignedWish<S>) -> Result<(), KVError>;
    fn update_high_qc(&mut self, qc: BlockQC<S>) -> Result<(), KVError>;
    fn update_locked_qc(&mut self, qc: BlockQC<S>) -> Result<(), KVError>;
    fn update_voted_view(&mut self, view: View) -> Result<(), KVError>;
    fn update_exec_height(&mut self, height: u64) -> Result<(), KVError>;
    fn update_exec_hash(&mut self, hash: S::Digest) -> Result<(), KVError>;
    fn update_cur_view(&mut self, view: View) -> Result<(), KVError>;
    fn update_cur_leader(&mut self, leader: NodeID) -> Result<(), KVError>;
    fn update_proposed_view(&mut self, view: View) -> Result<(), KVError>;
    fn update_scheduled_epoch(&mut self, epoch: View) -> Result<(), KVError>;
    fn update_lock_expired(&mut self, view: View) -> Result<(), KVError>;
    fn insert_status(&mut self, node_id: NodeID, view: View) -> Result<(), KVError>;
    fn clear_statuses(&mut self, view: View) -> Result<(), KVError>;
    fn insert_wish(&mut self, view: View) -> Result<(), KVError>;
    fn insert_relayed_tc(&mut self, view: View) -> Result<(), KVError>;
}

pub trait ReadWriteStorage<S: Scheme, C: Command>: ReadStorage<S, C> + WriteStorage<S, C> {}

impl<S: Scheme, C: Command, T: ReadStorage<S, C> + WriteStorage<S, C>> ReadWriteStorage<S, C> for T {}
