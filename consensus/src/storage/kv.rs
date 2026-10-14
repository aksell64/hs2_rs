use crate::types::View;
use crate::types::block::{Block, Command};
use crate::types::certificate::CertifiedBlock;
use hotstuff_crypto::sign::Scheme;
use hotstuff_types::node::NodeID;
use std::fmt::{self, Display};

pub struct KVItem<K, V> {
    pub key: K,
    pub value: V,
}

#[derive(Debug)]
pub enum KVError {
    Backend(Box<dyn std::error::Error + Send + Sync>),
    /// Stored bytes did not decode. Kept apart from a missing key on purpose: a replica
    /// that reads corruption as absence votes against its own history. Halt instead.
    Corruption {
        key: Vec<u8>,
    },
    Missing {
        key: Vec<u8>,
    },
    Closed,
}

impl Display for KVError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Backend(err) => write!(f, "storage backend failed: {err}"),
            Self::Corruption { key } => {
                f.write_str("stored value does not decode, key ")?;
                for byte in key {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
            Self::Missing { key } => {
                f.write_str("required value is missing, key ")?;
                for byte in key {
                    write!(f, "{byte:02x}")?;
                }
                Ok(())
            }
            Self::Closed => f.write_str("the store is closed"),
        }
    }
}

impl std::error::Error for KVError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(err) => Some(&**err),
            Self::Corruption { .. } | Self::Missing { .. } | Self::Closed => None,
        }
    }
}

pub trait KV: Send + Sync + 'static {
    type Key;
    type Value;
    type Batch: KVBatch<Self::Key, Self::Value>;

    fn get(&self, key: &Self::Key) -> Result<Option<Self::Value>, KVError>;
    fn set(&self, key: Self::Key, value: Self::Value) -> Result<(), KVError>;
    fn remove(&self, key: &Self::Key) -> Result<(), KVError>;
    fn range(
        &self,
        from: &Self::Key,
        to: &Self::Key,
    ) -> Result<impl Iterator<Item = Result<KVItem<Self::Key, Self::Value>, KVError>>, KVError>;

    fn batch(&self) -> Self::Batch;
    fn commit(&self, batch: Self::Batch) -> Result<(), KVError>;
}

pub trait KVBatch<K, V> {
    fn set(&mut self, key: K, value: V);
    fn remove(&mut self, key: K);
}

pub trait ReadStorage<S: Scheme, C: Command> {
    fn get_block_by_hash(&self, hash: &S::Digest) -> Result<Option<Block<S, C>>, KVError>;
    fn get_cert_by_view(&self, view: View) -> Result<Option<CertifiedBlock<S>>, KVError>;
    fn get_high_qc(&self) -> Result<CertifiedBlock<S>, KVError>;
    fn get_locked_qc(&self) -> Result<CertifiedBlock<S>, KVError>;
    fn get_voted_view(&self) -> Result<View, KVError>;
    fn get_exec_height(&self) -> Result<u64, KVError>;
    fn get_exec_hash(&self) -> Result<S::Digest, KVError>;
    fn get_cur_view(&self) -> Result<View, KVError>;
    fn get_cur_leader(&self) -> Result<NodeID, KVError>;
    fn get_proposed(&self) -> Result<bool, KVError>;
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

    fn get_parents_branch(
        &self,
        block: &Block<S, C>,
        depth: usize,
    ) -> Result<Vec<Block<S, C>>, KVError> {
        match self.get_block_by_hash(&block.parent)? {
            Some(parent) => self.get_block_branch(parent, depth),
            None => Ok(Vec::new()),
        }
    }
}

pub trait Storage<S: Scheme, C: Command>: ReadStorage<S, C> {
    type WriteBatch: WriteStorage<S, C>;

    fn new_write_batch(&self) -> Self::WriteBatch;
    fn commit_write_batch(&mut self, batch: Self::WriteBatch) -> Result<(), KVError>;
}

pub trait WriteStorage<S: Scheme, C: Command> {
    fn insert_block(&mut self, block: &Block<S, C>) -> Result<(), KVError>;
    fn commit_block(&mut self, block: &Block<S, C>) -> Result<(), KVError>;
    fn update_high_qc(&mut self, qc: CertifiedBlock<S>) -> Result<(), KVError>;
    fn update_locked_qc(&mut self, qc: CertifiedBlock<S>) -> Result<(), KVError>;
    fn update_voted_view(&mut self, view: View) -> Result<(), KVError>;
    fn update_exec_height(&mut self, height: u64) -> Result<(), KVError>;
    fn update_exec_hash(&mut self, hash: S::Digest) -> Result<(), KVError>;
    fn update_cur_view(&mut self, view: View) -> Result<(), KVError>;
    fn update_cur_leader(&mut self, leader: NodeID) -> Result<(), KVError>;
    fn update_proposed(&mut self, proposed: bool) -> Result<(), KVError>;
}

pub trait ReadWriteStorage<S: Scheme, C: Command>: ReadStorage<S, C> + WriteStorage<S, C> {}

impl<S: Scheme, C: Command, T: ReadStorage<S, C> + WriteStorage<S, C>> ReadWriteStorage<S, C> for T {}
