use crate::storage::kv::{KV, KVBatch};
use crate::storage::{KVError, ReadStorage, Storage, WriteStorage};
use crate::types::View;
use crate::types::block::{Block, Command};
use crate::types::certificate::BlockQC;
use codec::{Decode, DecodeAll, Encode};
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;

const BLOCK: u8 = 0;
const CERT: u8 = 1;
const COMMITTED: u8 = 2;
const HIGH_QC: u8 = 3;
const LOCKED_QC: u8 = 4;
const VOTED_VIEW: u8 = 5;
const EXEC_HEIGHT: u8 = 6;
const EXEC_HASH: u8 = 7;
const CUR_VIEW: u8 = 8;
const CUR_LEADER: u8 = 9;
const PROPOSED: u8 = 10;

fn key(tag: u8, suffix: &[u8]) -> Vec<u8> {
    let mut key = Vec::with_capacity(1 + suffix.len());
    key.push(tag);
    key.extend_from_slice(suffix);
    key
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

    fn get_cert_by_view(&self, view: View) -> Result<Option<BlockQC<S>>, KVError> {
        self.get(key(CERT, &view.to_be_bytes()))
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

    fn get_proposed(&self) -> Result<bool, KVError> {
        self.get_or_missing(key(PROPOSED, &[]))
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

    fn update_proposed(&mut self, proposed: bool) -> Result<(), KVError> {
        self.put(key(PROPOSED, &[]), proposed)
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
        write.update_proposed(false).unwrap();
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
        assert!(!read.get_proposed().unwrap());
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
