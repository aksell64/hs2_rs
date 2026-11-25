use crate::protocol::driver::DriverInput;
use crate::storage::kv::{KV, KVBatch};
use crate::storage::{KVError, ReadStorage, Storage, WriteStorage};
use crate::types::View;
use crate::types::block::{Block, Command};
use crate::types::certificate::{BlockQC, Signature, SignedVote, SignedWish, TimeoutQC};
use codec::{Decode, DecodeAll, Encode};
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;

const BLOCK: u8 = 0;
const BLOCK_QC: u8 = 1;
const COMMITTED: u8 = 2;
const HIGH_QC: u8 = 3;
const LOCKED_QC: u8 = 4;
const VOTED_VIEW: u8 = 5;
const EXEC_HEIGHT: u8 = 6;
const EXEC_HASH: u8 = 7;
const CUR_VIEW: u8 = 8;
const CUR_LEADER: u8 = 9;
const PROPOSED_VIEW: u8 = 10;
const STATUS: u8 = 11;
const SENT_WISH: u8 = 12;
const RELAYED_TC: u8 = 13;
const SCHEDULED_EPOCH: u8 = 14;
const LOCK_EXPIRED: u8 = 15;
const TIMEOUT_QC: u8 = 16;
const VOTE: u8 = 17;
const WISH_VOTE: u8 = 18;
const VOTE_BLOCK: u8 = 19;
const WISH_VOTE_SENDER: u8 = 20;
const NEXT_INPUT_ID: u8 = 21;
const INPUT_BY_VIEW: u8 = 22;
const INPUT_BY_BLOCK: u8 = 23;

fn key(tag: u8, suffix: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(1 + suffix.len());
    key.push(tag);
    key.extend_from_slice(suffix);
    key
}

fn view_node_key(tag: u8, view: View, node_id: NodeID) -> Vec<u8> {
    let mut key = key(tag, &view.to_be_bytes());
    key.extend_from_slice(&node_id.0.to_be_bytes());
    key
}

fn view_range(tag: u8, view: View) -> (Vec<u8>, Vec<u8>) {
    let from = key(tag, &view.to_be_bytes());
    let to = match view.checked_add(1) {
        Some(next_view) => key(tag, &next_view.to_be_bytes()),
        None => key(tag + 1, &[]),
    };
    (from, to)
}

fn input_key(tag: u8, group: &[u8], id: u64) -> Vec<u8> {
    let mut key = key(tag, group);
    key.extend_from_slice(&id.to_be_bytes());
    key
}

fn block_input_range(hash: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let from = key(INPUT_BY_BLOCK, &hash.encode());
    let mut to = from.clone();
    to.extend_from_slice(&u64::MAX.to_be_bytes());
    to.push(0);
    (from, to)
}

pub struct KVStorage<K> {
    kv: K,
}

impl<K: KV<Key = Vec<u8>, Value = Vec<u8>>> KVStorage<K> {
    pub fn new(kv: K) -> Self {
        Self { kv }
    }

    fn get<T: Decode>(&self, key: Vec<u8>) -> Result<Option<T>, KVError> {
        match self.kv.get(&key)? {
            None => Ok(None),
            Some(bytes) => T::decode_all(&mut &bytes[..])
                .map(Some)
                .map_err(|_| KVError::Corruption { key }),
        }
    }

    fn get_or_missing<T: Decode>(&self, key: Vec<u8>) -> Result<T, KVError> {
        self.get(key.clone())?.ok_or(KVError::Missing { key })
    }

    fn get_signatures<S: Scheme, T: Decode>(
        &self,
        tag: u8,
        view: View,
    ) -> Result<Vec<Signature<S, T>>, KVError> {
        let (from, to) = view_range(tag, view);
        let mut signatures = Vec::new();
        for item in self.kv.range(&from, &to)? {
            let item = item?;
            let signature = Signature::<S, T>::decode_all(&mut &item.value[..])
                .map_err(|_| KVError::Corruption {
                    key: item.key.clone(),
                })?;
            if item.key != view_node_key(tag, view, signature.creator()) {
                return Err(KVError::Corruption { key: item.key });
            }
            signatures.push(signature);
        }
        signatures.sort_by_key(|signature| signature.creator());
        Ok(signatures)
    }
}

impl<S, C, K> ReadStorage<S, C> for KVStorage<K>
where
    S: Scheme,
    C: Command,
    K: KV<Key = Vec<u8>, Value = Vec<u8>>,
{
    fn get_block_by_hash(&self, hash: &S::Digest) -> Result<Option<Block<S, C>>, KVError> {
        self.get(key(BLOCK, hash.as_ref()))
    }

    fn get_block_qc(&self, view: View) -> Result<Option<BlockQC<S>>, KVError> {
        self.get(key(BLOCK_QC, &view.to_be_bytes()))
    }

    fn get_timeout_qc(&self, view: View) -> Result<Option<TimeoutQC<S>>, KVError> {
        self.get(key(TIMEOUT_QC, &view.to_be_bytes()))
    }

    fn exists_vote(&self, view: View, sender: NodeID) -> Result<bool, KVError> {
        Ok(self
            .get::<SignedVote<S>>(view_node_key(VOTE, view, sender))?
            .is_some())
    }

    fn get_votes(&self, view: View) -> Result<Vec<SignedVote<S>>, KVError> {
        self.get_signatures(VOTE, view)
    }

    fn get_votes_count(&self, view: View, block: &S::Digest) -> Result<usize, KVError> {
        let (from, to) = view_range(VOTE_BLOCK, view);
        self.kv.range(&from, &to)?.try_fold(0, |count, item| {
            let item = item?;
            if item.key.len() != from.len() + size_of::<u32>() {
                return Err(KVError::Corruption { key: item.key });
            }
            let voted_block = S::Digest::decode_all(&mut &item.value[..])
                .map_err(|_| KVError::Corruption { key: item.key })?;
            Ok(count + usize::from(voted_block == *block))
        })
    }

    fn exists_wish_vote(&self, view: View, sender: NodeID) -> Result<bool, KVError> {
        Ok(self
            .get::<SignedWish<S>>(view_node_key(WISH_VOTE, view, sender))?
            .is_some())
    }

    fn get_wish_votes(&self, view: View) -> Result<Vec<SignedWish<S>>, KVError> {
        self.get_signatures(WISH_VOTE, view)
    }

    fn get_wish_votes_count(&self, view: View) -> Result<usize, KVError> {
        let (from, to) = view_range(WISH_VOTE_SENDER, view);
        self.kv.range(&from, &to)?.try_fold(0, |count, item| {
            let item = item?;
            if item.key.len() != from.len() + size_of::<u32>() || !item.value.is_empty() {
                return Err(KVError::Corruption { key: item.key });
            }
            Ok(count + 1)
        })
    }

    fn get_high_qc(&self) -> Result<BlockQC<S>, KVError> {
        self.get_or_missing(key(HIGH_QC, &[]))
    }

    fn get_locked_qc(&self) -> Result<BlockQC<S>, KVError> {
        self.get_or_missing(key(LOCKED_QC, &[]))
    }

    fn get_voted_view(&self) -> Result<View, KVError> {
        self.get_or_missing(key(VOTED_VIEW, &[]))
    }

    fn get_exec_height(&self) -> Result<u64, KVError> {
        self.get_or_missing(key(EXEC_HEIGHT, &[]))
    }

    fn get_exec_hash(&self) -> Result<S::Digest, KVError> {
        self.get_or_missing(key(EXEC_HASH, &[]))
    }

    fn get_cur_view(&self) -> Result<View, KVError> {
        self.get_or_missing(key(CUR_VIEW, &[]))
    }

    fn get_cur_leader(&self) -> Result<NodeID, KVError> {
        self.get_or_missing(key(CUR_LEADER, &[]))
    }

    fn get_proposed_view(&self) -> Result<View, KVError> {
        self.get_or_missing(key(PROPOSED_VIEW, &[]))
    }

    fn get_scheduled_epoch(&self) -> Result<Option<View>, KVError> {
        self.get(key(SCHEDULED_EPOCH, &[]))
    }

    fn get_lock_expired(&self) -> Result<View, KVError> {
        Ok(self.get(key(LOCK_EXPIRED, &[]))?.unwrap_or(0))
    }

    fn exists_wish(&self, view: View) -> Result<bool, KVError> {
        Ok(self.get::<()>(key(SENT_WISH, &view.to_be_bytes()))?.is_some())
    }

    fn exists_relayed_tc(&self, view: View) -> Result<bool, KVError> {
        Ok(self.get::<()>(key(RELAYED_TC, &view.to_be_bytes()))?.is_some())
    }

    fn get_statuses_count(&self, view: View) -> Result<usize, KVError> {
        let (from, to) = view_range(STATUS, view);
        self.kv.range(&from, &to)?.try_fold(0, |count, item| {
            let item = item?;
            if item.key.len() != from.len() + size_of::<u32>() || !item.value.is_empty() {
                return Err(KVError::Corruption { key: item.key });
            }
            Ok(count + 1)
        })
    }

    fn get_next_input_id(&self) -> Result<u64, KVError> {
        Ok(self.get(key(NEXT_INPUT_ID, &[]))?.unwrap_or(0))
    }

    fn get_inputs_up_to_view(
        &self,
        view: View,
    ) -> Result<Vec<(View, u64, DriverInput<S, C>)>, KVError> {
        let from = key(INPUT_BY_VIEW, &[]);
        let (_, to) = view_range(INPUT_BY_VIEW, view);
        let mut inputs = Vec::new();
        for item in self.kv.range(&from, &to)? {
            let item = item?;
            let (view, id): ([u8; 8], [u8; 8]) =
                DecodeAll::decode_all(&mut &item.key[1..]).map_err(|_| KVError::Corruption {
                    key: item.key.clone(),
                })?;
            let input = DriverInput::decode_all(&mut &item.value[..])
                .map_err(|_| KVError::Corruption { key: item.key })?;
            inputs.push((View::from_be_bytes(view), u64::from_be_bytes(id), input));
        }
        inputs.sort_by_key(|(view, id, _)| (*view, *id));
        Ok(inputs)
    }

    fn get_inputs_for_block(
        &self,
        hash: S::Digest,
    ) -> Result<Vec<(u64, DriverInput<S, C>)>, KVError> {
        let (from, to) = block_input_range(hash.as_ref());
        let mut inputs = Vec::new();
        for item in self.kv.range(&from, &to)? {
            let item = item?;
            let id: [u8; 8] = DecodeAll::decode_all(&mut &item.key[from.len()..]).map_err(|_| {
                KVError::Corruption {
                    key: item.key.clone(),
                }
            })?;
            let input = DriverInput::decode_all(&mut &item.value[..])
                .map_err(|_| KVError::Corruption { key: item.key })?;
            inputs.push((u64::from_be_bytes(id), input));
        }
        inputs.sort_by_key(|(id, _)| *id);
        Ok(inputs)
    }
}

impl<S, C, K> Storage<S, C> for KVStorage<K>
where
    S: Scheme,
    C: Command,
    K: KV<Key = Vec<u8>, Value = Vec<u8>>,
{
    type WriteBatch = KVWriteBatch<K::Batch>;

    fn new_write_batch(&self) -> Self::WriteBatch {
        KVWriteBatch(self.kv.batch())
    }

    fn commit_write_batch(&mut self, batch: Self::WriteBatch) -> Result<(), KVError> {
        self.kv.commit(batch.0)
    }

    fn has_status(&self, node_id: NodeID, view: View) -> Result<bool, KVError> {
        Ok(self.get::<()>(view_node_key(STATUS, view, node_id))?.is_some())
    }

    fn get_vote_block(&self, view: View, sender: NodeID) -> Result<Option<S::Digest>, KVError> {
        self.get(view_node_key(VOTE_BLOCK, view, sender))
    }
}

pub struct KVWriteBatch<B>(B);

impl<B: KVBatch<Vec<u8>, Vec<u8>>> KVWriteBatch<B> {
    fn put(&mut self, key: Vec<u8>, value: impl Encode) -> Result<(), KVError> {
        self.0.set(key, value.encode());
        Ok(())
    }
}

impl<S, C, B> WriteStorage<S, C> for KVWriteBatch<B>
where
    S: Scheme,
    C: Command,
    B: KVBatch<Vec<u8>, Vec<u8>>,
{
    fn insert_block(&mut self, block: &Block<S, C>) -> Result<(), KVError> {
        self.put(key(BLOCK, block.hash.as_ref()), block)
    }

    fn commit_block(&mut self, block: &Block<S, C>) -> Result<(), KVError> {
        self.put(key(COMMITTED, &block.height.to_be_bytes()), block.hash)
    }

    fn insert_block_qc(&mut self, qc: &BlockQC<S>) -> Result<(), KVError> {
        self.put(key(BLOCK_QC, &qc.view().to_be_bytes()), qc)
    }

    fn insert_timeout_qc(&mut self, qc: &TimeoutQC<S>) -> Result<(), KVError> {
        self.put(key(TIMEOUT_QC, &qc.view().to_be_bytes()), qc)
    }

    fn insert_vote(&mut self, vote: &SignedVote<S>) -> Result<(), KVError> {
        self.put(view_node_key(VOTE, vote.view(), vote.creator()), vote)?;
        self.put(
            view_node_key(VOTE_BLOCK, vote.view(), vote.creator()),
            vote.block_hash(),
        )
    }

    fn insert_wish_vote(&mut self, wish: &SignedWish<S>) -> Result<(), KVError> {
        self.put(view_node_key(WISH_VOTE, wish.view(), wish.creator()), wish)?;
        self.put(
            view_node_key(WISH_VOTE_SENDER, wish.view(), wish.creator()),
            (),
        )
    }

    fn update_high_qc(&mut self, qc: BlockQC<S>) -> Result<(), KVError> {
        self.put(key(HIGH_QC, &[]), qc)
    }

    fn update_locked_qc(&mut self, qc: BlockQC<S>) -> Result<(), KVError> {
        self.put(key(LOCKED_QC, &[]), qc)
    }

    fn update_voted_view(&mut self, view: View) -> Result<(), KVError> {
        self.put(key(VOTED_VIEW, &[]), view)
    }

    fn update_exec_height(&mut self, height: u64) -> Result<(), KVError> {
        self.put(key(EXEC_HEIGHT, &[]), height)
    }

    fn update_exec_hash(&mut self, hash: S::Digest) -> Result<(), KVError> {
        self.put(key(EXEC_HASH, &[]), hash)
    }

    fn update_cur_view(&mut self, view: View) -> Result<(), KVError> {
        self.put(key(CUR_VIEW, &[]), view)
    }

    fn update_cur_leader(&mut self, leader: NodeID) -> Result<(), KVError> {
        self.put(key(CUR_LEADER, &[]), leader)
    }

    fn update_proposed_view(&mut self, view: View) -> Result<(), KVError> {
        self.put(key(PROPOSED_VIEW, &[]), view)
    }

    fn update_scheduled_epoch(&mut self, epoch: View) -> Result<(), KVError> {
        self.put(key(SCHEDULED_EPOCH, &[]), epoch)
    }

    fn update_lock_expired(&mut self, view: View) -> Result<(), KVError> {
        self.put(key(LOCK_EXPIRED, &[]), view)
    }

    fn insert_status(&mut self, node_id: NodeID, view: View) -> Result<(), KVError> {
        self.put(view_node_key(STATUS, view, node_id), ())
    }

    fn clear_statuses(&mut self, view: View) -> Result<(), KVError> {
        let (from, to) = view_range(STATUS, view);
        self.0.remove_range(from, to);
        Ok(())
    }

    fn insert_wish(&mut self, view: View) -> Result<(), KVError> {
        self.put(key(SENT_WISH, &view.to_be_bytes()), ())
    }

    fn insert_relayed_tc(&mut self, view: View) -> Result<(), KVError> {
        self.put(key(RELAYED_TC, &view.to_be_bytes()), ())
    }

    fn update_next_input_id(&mut self, id: u64) -> Result<(), KVError> {
        self.put(key(NEXT_INPUT_ID, &[]), id)
    }

    fn insert_input_for_view(
        &mut self,
        view: View,
        id: u64,
        input: &DriverInput<S, C>,
    ) -> Result<(), KVError> {
        self.put(input_key(INPUT_BY_VIEW, &view.to_be_bytes(), id), input)
    }

    fn insert_input_for_block(
        &mut self,
        hash: S::Digest,
        id: u64,
        input: &DriverInput<S, C>,
    ) -> Result<(), KVError> {
        self.put(input_key(INPUT_BY_BLOCK, &hash.as_ref().encode(), id), input)
    }

    fn clear_inputs_up_to_view(&mut self, view: View) -> Result<(), KVError> {
        let from = key(INPUT_BY_VIEW, &[]);
        let (_, to) = view_range(INPUT_BY_VIEW, view);
        self.0.remove_range(from, to);
        Ok(())
    }

    fn clear_inputs_for_block(&mut self, hash: S::Digest) -> Result<(), KVError> {
        let (from, to) = block_input_range(hash.as_ref());
        self.0.remove_range(from, to);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mocks::kv::MemoryKV;
    use crate::mocks::scheme::{TestCommand, TestScheme, block, digest, qc};

    type Store = KVStorage<MemoryKV<Vec<u8>, Vec<u8>>>;

    fn genesis_store() -> (TestScheme, Store) {
        let scheme = TestScheme { n: 4, id: None };
        let genesis_qc = qc(&scheme, 0, digest(0));
        let genesis = block(digest(0), digest(0), genesis_qc.clone(), 0);
        let mut store: Store = KVStorage::new(MemoryKV::new());
        let mut batch = Storage::<TestScheme, TestCommand>::new_write_batch(&store);
        let write: &mut dyn WriteStorage<TestScheme, TestCommand> = &mut batch;
        write.insert_block(&genesis).unwrap();
        write.commit_block(&genesis).unwrap();
        write.update_high_qc(genesis_qc.clone()).unwrap();
        write.update_locked_qc(genesis_qc).unwrap();
        write.update_voted_view(0).unwrap();
        write.update_exec_height(0).unwrap();
        write.update_exec_hash(digest(0)).unwrap();
        write.update_cur_view(0).unwrap();
        write.update_cur_leader(NodeID::from(0)).unwrap();
        write.update_proposed_view(0).unwrap();
        Storage::<TestScheme, TestCommand>::commit_write_batch(&mut store, batch).unwrap();
        (scheme, store)
    }

    #[test]
    fn genesis_state_round_trips() {
        let (_, store) = genesis_store();
        let read: &dyn ReadStorage<TestScheme, TestCommand> = &store;
        assert_eq!(read.get_high_qc().unwrap().view(), 0);
        assert_eq!(read.get_locked_qc().unwrap().block(), digest(0));
        assert_eq!(read.get_voted_view().unwrap(), 0);
        assert_eq!(read.get_exec_height().unwrap(), 0);
        assert_eq!(read.get_exec_hash().unwrap(), digest(0));
        assert_eq!(read.get_cur_view().unwrap(), 0);
        assert_eq!(read.get_proposed_view().unwrap(), 0);
        assert_eq!(read.get_block_by_hash(&digest(0)).unwrap().unwrap().height, 0);
    }

    #[test]
    fn batch_is_visible_only_after_commit() {
        let (scheme, mut store) = genesis_store();
        let b1 = block(digest(1), digest(0), qc(&scheme, 0, digest(0)), 1);
        let mut batch = Storage::<TestScheme, TestCommand>::new_write_batch(&store);
        WriteStorage::<TestScheme, TestCommand>::insert_block(&mut batch, &b1).unwrap();
        WriteStorage::<TestScheme, TestCommand>::update_voted_view(&mut batch, 1).unwrap();
        WriteStorage::<TestScheme, TestCommand>::update_locked_qc(&mut batch, qc(&scheme, 1, digest(1))).unwrap();

        let read: &dyn ReadStorage<TestScheme, TestCommand> = &store;
        assert!(read.get_block_by_hash(&digest(1)).unwrap().is_none());
        assert_eq!(read.get_voted_view().unwrap(), 0);

        Storage::<TestScheme, TestCommand>::commit_write_batch(&mut store, batch).unwrap();
        let read: &dyn ReadStorage<TestScheme, TestCommand> = &store;
        let stored = read.get_block_by_hash(&digest(1)).unwrap().unwrap();
        assert_eq!(stored.parent, digest(0));
        assert_eq!(stored.cmd, TestCommand(1));
        assert_eq!(read.get_voted_view().unwrap(), 1);
        assert_eq!(read.get_locked_qc().unwrap().view(), 1);
    }

    #[test]
    fn missing_and_corrupted_values_are_errors() {
        let store: Store = KVStorage::new(MemoryKV::new());
        let read: &dyn ReadStorage<TestScheme, TestCommand> = &store;
        assert!(matches!(read.get_voted_view(), Err(KVError::Missing { .. })));

        store.kv.set(key(VOTED_VIEW, &[]), vec![1, 2, 3]).unwrap();
        let read: &dyn ReadStorage<TestScheme, TestCommand> = &store;
        assert!(matches!(read.get_voted_view(), Err(KVError::Corruption { .. })));
    }
}
