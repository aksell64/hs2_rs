use crate::storage::kv::{KV, KVBatch, KVError, KVItem};
use std::collections::BTreeMap;
use std::sync::{RwLock, RwLockReadGuard, RwLockWriteGuard};

pub struct MemoryKV<K, V> {
    map: RwLock<BTreeMap<K, V>>,
}

impl<K: Ord, V> MemoryKV<K, V> {
    pub fn new() -> Self {
        Self {
            map: RwLock::new(BTreeMap::new()),
        }
    }

    fn read(&self) -> RwLockReadGuard<'_, BTreeMap<K, V>> {
        self.map.read().unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn write(&self) -> RwLockWriteGuard<'_, BTreeMap<K, V>> {
        self.map.write().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl<K: Ord, V> Default for MemoryKV<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

pub enum MemoryOp<K, V> {
    Set(K, V),
    Remove(K),
}

pub struct MemoryBatch<K, V> {
    ops: Vec<MemoryOp<K, V>>,
}

impl<K, V> KVBatch<K, V> for MemoryBatch<K, V> {
    fn set(&mut self, key: K, value: V) {
        self.ops.push(MemoryOp::Set(key, value));
    }

    fn remove(&mut self, key: K) {
        self.ops.push(MemoryOp::Remove(key));
    }
}

impl<K, V> KV for MemoryKV<K, V>
where
    K: Ord + Clone + Send + Sync + 'static,
    V: Clone + Send + Sync + 'static,
{
    type Key = K;
    type Value = V;
    type Batch = MemoryBatch<K, V>;

    fn get(&self, key: &K) -> Result<Option<V>, KVError> {
        Ok(self.read().get(key).cloned())
    }

    fn set(&self, key: K, value: V) -> Result<(), KVError> {
        self.write().insert(key, value);
        Ok(())
    }

    fn remove(&self, key: &K) -> Result<(), KVError> {
        self.write().remove(key);
        Ok(())
    }

    fn range(
        &self,
        from: &K,
        to: &K,
    ) -> Result<impl Iterator<Item = Result<KVItem<K, V>, KVError>>, KVError> {
        if from >= to {
            return Ok(Vec::new().into_iter());
        }
        let items: Vec<_> = self
            .read()
            .range(from.clone()..to.clone())
            .map(|(key, value)| {
                Ok(KVItem {
                    key: key.clone(),
                    value: value.clone(),
                })
            })
            .collect();
        Ok(items.into_iter())
    }

    fn batch(&self) -> MemoryBatch<K, V> {
        MemoryBatch { ops: Vec::new() }
    }

    fn commit(&self, batch: MemoryBatch<K, V>) -> Result<(), KVError> {
        let mut map = self.write();
        for op in batch.ops {
            match op {
                MemoryOp::Set(key, value) => {
                    map.insert(key, value);
                }
                MemoryOp::Remove(key) => {
                    map.remove(&key);
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(kv: &MemoryKV<u32, u32>, from: u32, to: u32) -> Vec<u32> {
        kv.range(&from, &to).unwrap().map(|item| item.unwrap().key).collect()
    }

    #[test]
    fn set_get_remove() {
        let kv = MemoryKV::new();
        kv.set(1u32, 10u32).unwrap();
        assert_eq!(kv.get(&1).unwrap(), Some(10));
        kv.remove(&1).unwrap();
        assert_eq!(kv.get(&1).unwrap(), None);
    }

    #[test]
    fn range_is_half_open_and_ordered() {
        let kv = MemoryKV::new();
        for key in [5u32, 1, 3, 7] {
            kv.set(key, key).unwrap();
        }
        assert_eq!(keys(&kv, 1, 7), vec![1, 3, 5]);
        assert_eq!(keys(&kv, 7, 1), Vec::<u32>::new());
    }

    #[test]
    fn batch_is_invisible_until_commit_and_applies_in_order() {
        let kv = MemoryKV::new();
        kv.set(1u32, 1u32).unwrap();
        let mut batch = kv.batch();
        batch.set(2, 2);
        batch.remove(1);
        batch.set(2, 3);
        assert_eq!(kv.get(&2).unwrap(), None);
        assert_eq!(kv.get(&1).unwrap(), Some(1));
        kv.commit(batch).unwrap();
        assert_eq!(kv.get(&2).unwrap(), Some(3));
        assert_eq!(kv.get(&1).unwrap(), None);
    }
}
