#![forbid(unsafe_code)]

use std::{
    borrow::Borrow,
    hash::{BuildHasher, Hash, RandomState},
};

use hashbrown::HashTable;

#[allow(clippy::type_complexity)]
type TablePointer<T> = dbuf::triomphe::OffsetArc<
    dbuf::raw::DoubleBufferData<
        HashTable<T>,
        dbuf::strategy::flashmap::FlashStrategy<dbuf::strategy::flashmap::AdaptiveParkToken>,
    >,
>;

pub struct Writer<'env, K, V, S = RandomState> {
    writer: dbuf::op::OpWriter<TablePointer<(K, V)>, HashTableOperation<'env, K, V, S>>,
    hasher: S,
}

pub struct Reader<K, V> {
    reader: dbuf::raw::Reader<TablePointer<(K, V)>>,
}

pub enum HashTableOperation<'env, K, V, S> {
    Insert {
        key: K,
        value: V,
    },
    Remove {
        key: K,
    },
    Custom {
        f: Box<dyn FnMut(bool, &mut HashTable<(K, V)>, &S) + Send + 'env>,
    },
}

impl<'env, K, V> Writer<'env, K, V> {
    pub fn new() -> Self {
        Self::with_hasher(RandomState::new())
    }
}

impl<'env, K, V, S> Writer<'env, K, V, S> {
    pub fn with_hasher(hasher: S) -> Self {
        Self {
            writer: dbuf::op::OpWriter::from(dbuf::raw::Writer::new(
                dbuf::triomphe::UniqueArc::new(dbuf::raw::DoubleBufferData::new(
                    HashTable::new(),
                    HashTable::new(),
                    dbuf::strategy::flashmap::FlashStrategy::new(),
                )),
            )),
            hasher,
        }
    }

    pub fn insert(&mut self, key: K, value: V)
    where
        K: Hash + Eq + Clone,
        V: Clone,
        S: BuildHasher,
    {
        self.writer.push(HashTableOperation::Insert { key, value })
    }

    pub fn remove(&mut self, key: K)
    where
        K: Hash + Eq + Clone,
        V: Clone,
        S: BuildHasher,
    {
        self.writer.push(HashTableOperation::Remove { key })
    }

    pub fn get_entry<Q: ?Sized>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        let map = self.writer.get();
        let hash = self.hasher.hash_one(key);
        let (k, v) = map.find(hash, |(k, _)| k.borrow() == key)?;
        Some((k, v))
    }

    pub fn get<Q: ?Sized>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: Hash + Eq,
        S: BuildHasher,
    {
        self.get_entry(key).map(|(_, value)| value)
    }

    pub fn retain(&mut self, mut f: impl FnMut(&K, &mut V) -> bool + Send + 'env)
    where
        K: Hash + Eq + Clone,
        V: Clone,
        S: BuildHasher,
    {
        self.writer.push(HashTableOperation::Custom {
            f: Box::new(move |_, table, hasher| table.retain(|(key, value)| f(key, value))),
        })
    }

    pub fn publish(&mut self)
    where
        K: Hash + Eq + Clone,
        V: Clone,
        S: BuildHasher,
    {
        self.writer.swap_buffers(&mut self.hasher);
    }
}

impl<K, V, S: Default> Default for Writer<'_, K, V, S> {
    fn default() -> Self {
        Self::with_hasher(Default::default())
    }
}

impl<K: Hash + Eq + Clone, V: Clone, S: BuildHasher> dbuf::op::Operation<HashTable<(K, V)>, S>
    for HashTableOperation<'_, K, V, S>
{
    fn apply_once(mut self, buffer: &mut HashTable<(K, V)>, hasher: &mut S) {
        match self {
            HashTableOperation::Insert { key, value } => {
                let hash = hasher.hash_one(&key);
                if let Some(old_entry) = buffer.find_mut(hash, |k| k.0 == key) {
                    *old_entry = (key, value);
                } else {
                    buffer.insert_unique(hash, (key, value), |(key, _)| hasher.hash_one(key));
                }
            }
            HashTableOperation::Remove { key } => {
                let hash = hasher.hash_one(&key);
                if let Ok(entry) = buffer.find_entry(hash, |(k, _)| *k == key) {
                    entry.remove();
                }
            }
            HashTableOperation::Custom { mut f } => f(false, buffer, hasher),
        }
    }

    fn apply(&mut self, buffer: &mut HashTable<(K, V)>, hasher: &mut S) {
        match self {
            HashTableOperation::Insert { key, value } => {
                let hash = hasher.hash_one(&*key);
                if let Some(old_entry) = buffer.find_mut(hash, |k| k.0 == *key) {
                    old_entry.0.clone_from(key);
                    old_entry.1.clone_from(value);
                } else {
                    buffer.insert_unique(hash, (key.clone(), value.clone()), |(key, _)| {
                        hasher.hash_one(key)
                    });
                }
            }
            HashTableOperation::Remove { key } => {
                let hash = hasher.hash_one(&*key);
                if let Ok(entry) = buffer.find_entry(hash, |(k, _)| k == key) {
                    entry.remove();
                }
            }
            HashTableOperation::Custom { f } => f(true, buffer, hasher),
        }
    }
}
