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
