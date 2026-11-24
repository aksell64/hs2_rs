use crate::storage::{KVError, ReadStorage, Storage, WriteStorage};
use crate::types::View;
use crate::types::block::{Block, Command};
use crate::types::certificate::{BlockQC, SignedVote, SignedWish, TimeoutQC};
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

pub struct StagedStorage<'a, S: Scheme, C: Command, DB: Storage<S, C>> {
    storage: &'a DB,
    batch: DB::WriteBatch,
    blocks: HashMap<S::Digest, Block<S, C>>,
    block_qcs: HashMap<View, BlockQC<S>>,
    timeout_qcs: HashMap<View, TimeoutQC<S>>,
    votes: HashMap<View, BTreeMap<NodeID, SignedVote<S>>>,
    wish_votes: HashMap<View, BTreeMap<NodeID, SignedWish<S>>>,
    high_qc: Option<BlockQC<S>>,
    locked_qc: Option<BlockQC<S>>,
    voted_view: Option<View>,
    exec_height: Option<u64>,
    exec_hash: Option<S::Digest>,
    cur_view: Option<View>,
    cur_leader: Option<NodeID>,
    proposed_view: Option<View>,
    scheduled_epoch: Option<View>,
    lock_expired: Option<View>,
    statuses: HashMap<View, BTreeSet<NodeID>>,
    cleared_statuses: HashSet<View>,
    wishes: HashSet<View>,
    relayed_tc: HashSet<View>,
}

impl<'a, S: Scheme, C: Command, DB: Storage<S, C>> StagedStorage<'a, S, C, DB> {
    pub fn new(storage: &'a DB) -> Self {
        Self {
            storage,
            batch: storage.new_write_batch(),
            blocks: HashMap::new(),
            block_qcs: HashMap::new(),
            timeout_qcs: HashMap::new(),
            votes: HashMap::new(),
            wish_votes: HashMap::new(),
            high_qc: None,
            locked_qc: None,
            voted_view: None,
            exec_height: None,
            exec_hash: None,
            cur_view: None,
            cur_leader: None,
            proposed_view: None,
            scheduled_epoch: None,
            lock_expired: None,
            statuses: HashMap::new(),
            cleared_statuses: HashSet::new(),
            wishes: HashSet::new(),
            relayed_tc: HashSet::new(),
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

    fn get_block_qc(&self, view: View) -> Result<Option<BlockQC<S>>, KVError> {
        match self.block_qcs.get(&view) {
            Some(qc) => Ok(Some(qc.clone())),
            None => self.storage.get_block_qc(view),
        }
    }

    fn get_timeout_qc(&self, view: View) -> Result<Option<TimeoutQC<S>>, KVError> {
        match self.timeout_qcs.get(&view) {
            Some(qc) => Ok(Some(qc.clone())),
            None => self.storage.get_timeout_qc(view),
        }
    }

    fn exists_vote(&self, view: View, sender: NodeID) -> Result<bool, KVError> {
        Ok(self
            .votes
            .get(&view)
            .is_some_and(|votes| votes.contains_key(&sender))
            || self.storage.exists_vote(view, sender)?)
    }

    fn get_votes(&self, view: View) -> Result<Vec<SignedVote<S>>, KVError> {
        let mut votes: BTreeMap<_, _> = self
            .storage
            .get_votes(view)?
            .into_iter()
            .map(|vote| (vote.creator(), vote))
            .collect();
        if let Some(pending) = self.votes.get(&view) {
            votes.extend(pending.iter().map(|(&sender, vote)| (sender, vote.clone())));
        }
        Ok(votes.into_values().collect())
    }

    fn get_votes_count(&self, view: View, block: &S::Digest) -> Result<usize, KVError> {
        let mut count = self.storage.get_votes_count(view, block)?;
        if let Some(pending) = self.votes.get(&view) {
            for (&sender, vote) in pending {
                if self.storage.get_vote_block(view, sender)?.as_ref() == Some(block) {
                    count -= 1;
                }
                if vote.block_hash() == *block {
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    fn exists_wish_vote(&self, view: View, sender: NodeID) -> Result<bool, KVError> {
        Ok(self
            .wish_votes
            .get(&view)
            .is_some_and(|wishes| wishes.contains_key(&sender))
            || self.storage.exists_wish_vote(view, sender)?)
    }

    fn get_wish_votes(&self, view: View) -> Result<Vec<SignedWish<S>>, KVError> {
        let mut wishes: BTreeMap<_, _> = self
            .storage
            .get_wish_votes(view)?
            .into_iter()
            .map(|wish| (wish.creator(), wish))
            .collect();
        if let Some(pending) = self.wish_votes.get(&view) {
            wishes.extend(pending.iter().map(|(&sender, wish)| (sender, wish.clone())));
        }
        Ok(wishes.into_values().collect())
    }

    fn get_wish_votes_count(&self, view: View) -> Result<usize, KVError> {
        let mut count = self.storage.get_wish_votes_count(view)?;
        if let Some(pending) = self.wish_votes.get(&view) {
            for &sender in pending.keys() {
                if !self.storage.exists_wish_vote(view, sender)? {
                    count += 1;
                }
            }
        }
        Ok(count)
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

    fn get_proposed_view(&self) -> Result<View, KVError> {
        match self.proposed_view {
            Some(view) => Ok(view),
            None => self.storage.get_proposed_view(),
        }
    }

    fn get_scheduled_epoch(&self) -> Result<Option<View>, KVError> {
        match self.scheduled_epoch {
            Some(epoch) => Ok(Some(epoch)),
            None => self.storage.get_scheduled_epoch(),
        }
    }

    fn get_lock_expired(&self) -> Result<View, KVError> {
        match self.lock_expired {
            Some(view) => Ok(view),
            None => self.storage.get_lock_expired(),
        }
    }

    fn exists_wish(&self, view: View) -> Result<bool, KVError> {
        Ok(self.wishes.contains(&view) || self.storage.exists_wish(view)?)
    }

    fn exists_relayed_tc(&self, view: View) -> Result<bool, KVError> {
        Ok(self.relayed_tc.contains(&view) || self.storage.exists_relayed_tc(view)?)
    }

    fn get_statuses_count(&self, view: View) -> Result<usize, KVError> {
        let statuses = self.statuses.get(&view);
        if self.cleared_statuses.contains(&view) {
            return Ok(statuses.map_or(0, BTreeSet::len));
        }

        let mut count = self.storage.get_statuses_count(view)?;
        if let Some(statuses) = statuses {
            for &node_id in statuses {
                if !self.storage.has_status(node_id, view)? {
                    count += 1;
                }
            }
        }
        Ok(count)
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

    fn insert_block_qc(&mut self, qc: &BlockQC<S>) -> Result<(), KVError> {
        self.batch.insert_block_qc(qc)?;
        self.block_qcs.insert(qc.view(), qc.clone());
        Ok(())
    }

    fn insert_timeout_qc(&mut self, qc: &TimeoutQC<S>) -> Result<(), KVError> {
        self.batch.insert_timeout_qc(qc)?;
        self.timeout_qcs.insert(qc.view(), qc.clone());
        Ok(())
    }

    fn insert_vote(&mut self, vote: &SignedVote<S>) -> Result<(), KVError> {
        self.batch.insert_vote(vote)?;
        self.votes
            .entry(vote.view())
            .or_default()
            .insert(vote.creator(), vote.clone());
        Ok(())
    }

    fn insert_wish_vote(&mut self, wish: &SignedWish<S>) -> Result<(), KVError> {
        self.batch.insert_wish_vote(wish)?;
        self.wish_votes
            .entry(wish.view())
            .or_default()
            .insert(wish.creator(), wish.clone());
        Ok(())
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

    fn update_proposed_view(&mut self, view: View) -> Result<(), KVError> {
        self.proposed_view = Some(view);
        self.batch.update_proposed_view(view)
    }

    fn update_scheduled_epoch(&mut self, epoch: View) -> Result<(), KVError> {
        self.batch.update_scheduled_epoch(epoch)?;
        self.scheduled_epoch = Some(epoch);
        Ok(())
    }

    fn update_lock_expired(&mut self, view: View) -> Result<(), KVError> {
        self.batch.update_lock_expired(view)?;
        self.lock_expired = Some(view);
        Ok(())
    }

    fn insert_status(&mut self, node_id: NodeID, view: View) -> Result<(), KVError> {
        self.batch.insert_status(node_id, view)?;
        self.statuses.entry(view).or_default().insert(node_id);
        Ok(())
    }

    fn clear_statuses(&mut self, view: View) -> Result<(), KVError> {
        self.batch.clear_statuses(view)?;
        self.statuses.remove(&view);
        self.cleared_statuses.insert(view);
        Ok(())
    }

    fn insert_wish(&mut self, view: View) -> Result<(), KVError> {
        self.batch.insert_wish(view)?;
        self.wishes.insert(view);
        Ok(())
    }

    fn insert_relayed_tc(&mut self, view: View) -> Result<(), KVError> {
        self.batch.insert_relayed_tc(view)?;
        self.relayed_tc.insert(view);
        Ok(())
    }
}
