use crate::types::View;
use crate::types::block::{Block, Command};
use crate::types::certificate::CertifiedBlock;
use hotstuff_crypto::sign::Scheme;
use std::fmt::{self, Display};

struct KVItem<K, V> {
    key: K,
    value: V,
}

#[derive(Debug)]
pub enum KVError {
    Backend(Box<dyn std::error::Error + Send + Sync>),
    /// Stored bytes did not decode. Kept apart from a missing key on purpose: a replica
    /// that reads corruption as absence votes against its own history. Halt instead.
    Corruption {
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
            Self::Closed => f.write_str("the store is closed"),
        }
    }
}

impl std::error::Error for KVError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Backend(err) => Some(&**err),
            Self::Corruption { .. } | Self::Closed => None,
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


pub trait Storage<S: Scheme, C: Command> {
    fn get_block_by_hash(&self, hash: &S::Digest) -> Result<Option<Block<S, C>>, KVError>;
    fn get_cert_by_view(&self, view: View) -> Result<Option<CertifiedBlock<S>>, KVError>;
    fn insert_block(&mut self, block: Block<S, C>) -> Result<(), KVError>;
    fn commit_block(&mut self, block: Block<S, C>) -> Result<(), KVError>;

    fn get_block_branch(&self, block: Block<S, C>, depth: usize) -> Result<Vec<Block<S, C>>, KVError> {
        let mut branch = Vec::with_capacity(depth);
        let mut current = Some(block);
        while branch.len() < depth {
            let Some(block) = current else { break };
            current = self.get_block_by_hash(&block.parent)?;
            branch.push(block);
        }
        Ok(branch)
    }

    fn get_parents_branch(&self, block: &Block<S, C>, depth: usize) -> Result<Vec<Block<S, C>>, KVError> {
        match self.get_block_by_hash(&block.parent)? {
            Some(parent) => self.get_block_branch(parent, depth),
            None => Ok(Vec::new()),
        }
    }
}