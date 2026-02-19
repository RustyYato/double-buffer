#![forbid(unsafe_code)]

use std::{
    borrow::Borrow,
    hash::{BuildHasher, Hash, RandomState},
    ops::Deref,
};

use dbuf::interface::Strategy;
use hashbrown::HashTable;

pub type DefaultStrategy =
    dbuf::strategy::flashmap::FlashStrategy<dbuf::strategy::flash_park_token::AdaptiveParkToken>;

#[allow(clippy::type_complexity)]
type TablePointer<T, H, S> =
    dbuf::triomphe::OffsetArc<dbuf::raw::DoubleBufferData<HashTable<T>, S, H>>;

#[allow(clippy::type_complexity)]
pub struct Writer<'env, K, V, H = RandomState, S: Strategy = DefaultStrategy> {
    writer: dbuf::op::OpWriter<TablePointer<(K, V), H, S>, HashTableOperation<'env, K, V, H>>,
}

pub struct Reader<K, V, H = RandomState, S: Strategy = DefaultStrategy> {
    reader: dbuf::raw::Reader<TablePointer<(K, V), H, S>>,
}

#[allow(clippy::type_complexity)]
pub struct TableGuard<'a, K, V, H = RandomState, S: Strategy = DefaultStrategy> {
    reader: dbuf::raw::ReaderGuard<'a, HashTable<(K, V)>, TablePointer<(K, V), H, S>>,
}

pub struct ReadGuard<'a, T: ?Sized, K, V, H = RandomState, S: Strategy = DefaultStrategy> {
    reader: dbuf::raw::ReaderGuard<'a, T, TablePointer<(K, V), H, S>>,
}

impl<T: ?Sized, K, V, H, S: Strategy> Deref for ReadGuard<'_, T, K, V, H, S> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

pub enum HashTableOperation<'env, K, V, H> {
    Insert {
        key: K,
        value: V,
    },
    Remove {
        key: K,
    },
    #[allow(clippy::type_complexity)]
    Custom {
        f: Box<dyn FnMut(bool, &mut HashTable<(K, V)>, &H) + Send + 'env>,
    },
}

impl<K, V> Writer<'_, K, V> {
    pub fn new() -> Self {
        Self::with_hasher_and_strategy(RandomState::new(), DefaultStrategy::new())
    }
}

impl<K, V, H, S: Strategy> Writer<'_, K, V, H, S> {
    pub fn with_hasher_and_strategy(hasher: H, strategy: S) -> Self {
        Self {
            writer: dbuf::op::OpWriter::from(dbuf::raw::Writer::new(
                dbuf::triomphe::UniqueArc::new(dbuf::raw::DoubleBufferData::with_extras(
                    HashTable::new(),
                    HashTable::new(),
                    strategy,
                    hasher,
                )),
            )),
        }
    }

    pub fn reader(&self) -> Reader<K, V, H, S> {
        Reader {
            reader: self.writer.reader(),
        }
    }
}

impl<'env, K, V, H: BuildHasher, S: Strategy> Writer<'env, K, V, H, S> {
    pub fn insert(&mut self, key: K, value: V)
    where
        K: Hash + Eq + Clone,
        V: Clone,
    {
        self.writer.push(HashTableOperation::Insert { key, value })
    }

    pub fn remove(&mut self, key: K)
    where
        K: Hash + Eq + Clone,
        V: Clone,
    {
        self.writer.push(HashTableOperation::Remove { key })
    }

    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.get(key).is_some()
    }

    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        let map = self.writer.get();
        let hash = self.writer.extras().hash_one(key);
        let (k, v) = map.find(hash, |(k, _)| k.borrow() == key)?;
        Some((k, v))
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
        Q: ?Sized + Hash + Eq,
    {
        self.get_key_value(key).map(|(_, value)| value)
    }

    pub fn retain(&mut self, mut f: impl FnMut(&K, &mut V) -> bool + Send + 'env)
    where
        K: Hash + Eq + Clone,
        V: Clone,
    {
        self.writer.push(HashTableOperation::Custom {
            f: Box::new(move |_, table, _hasher| table.retain(|(key, value)| f(key, value))),
        })
    }

    pub fn publish(&mut self)
    where
        K: Hash + Eq + Clone,
        V: Clone,
        S: dbuf::interface::BlockingStrategy<SwapError = core::convert::Infallible>,
    {
        self.writer.swap_buffers(&mut ());
    }

    pub async fn apublish(&mut self)
    where
        K: Hash + Eq + Clone,
        V: Clone,
        S: dbuf::interface::AsyncStrategy<SwapError = core::convert::Infallible>,
    {
        self.writer.aswap_buffers(&mut ()).await;
    }
}

impl<K, V, S> Reader<K, V, S> {
    pub fn load(&mut self) -> TableGuard<'_, K, V, S> {
        TableGuard {
            reader: self.reader.read(),
        }
    }
}

impl<'a, K, V, S: BuildHasher> TableGuard<'a, K, V, S> {
    pub fn contains_key<Q>(&self, key: &Q) -> bool
    where
        Q: ?Sized + Hash + Eq,
        K: Borrow<Q>,
    {
        self.get(key).is_some()
    }

    pub fn get<Q>(&self, key: &Q) -> Option<&V>
    where
        Q: ?Sized + Hash + Eq,
        K: Borrow<Q>,
    {
        let hash = self.reader.extras().hash_one(key);

        match self.reader.find(hash, |(k, _)| k.borrow() == key) {
            Some((_, v)) => Some(v),
            None => None,
        }
    }

    pub fn get_key_value<Q>(&self, key: &Q) -> Option<(&K, &V)>
    where
        Q: ?Sized + Hash + Eq,
        K: Borrow<Q>,
    {
        let hash = self.reader.extras().hash_one(key);

        #[allow(clippy::manual_map)]
        match self.reader.find(hash, |(k, _)| k.borrow() == key) {
            Some((k, v)) => Some((k, v)),
            None => None,
        }
    }

    pub fn into_get<Q>(self, key: &Q) -> Result<ReadGuard<'a, V, K, V, S>, Self>
    where
        Q: ?Sized + Hash + Eq,
        K: Borrow<Q>,
    {
        let mapped_guard = self.reader.try_map_with_extras(|table, hasher| {
            let hash = hasher.hash_one(key);
            match table.find(hash, |(k, _)| k.borrow() == key) {
                Some((_, value)) => Ok(value),
                None => Err(()),
            }
        });

        match mapped_guard {
            Ok(reader) => Ok(ReadGuard { reader }),
            Err((reader, ())) => Err(TableGuard { reader }),
        }
    }
}

impl<K, V, S> TableGuard<'_, K, V, S> {
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            raw: self.reader.iter(),
        }
    }
}

impl<T: ?Sized, K, V, S> ReadGuard<'_, T, K, V, S> {}

pub struct Iter<'a, K, V> {
    raw: hashbrown::hash_table::Iter<'a, (K, V)>,
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);

    fn next(&mut self) -> Option<Self::Item> {
        let (k, v) = self.raw.next()?;
        Some((k, v))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.raw.size_hint()
    }
}

impl<K, V, H: Default, S: Strategy + Default> Default for Writer<'_, K, V, H, S> {
    fn default() -> Self {
        Self::with_hasher_and_strategy(Default::default(), Default::default())
    }
}

impl<K: Hash + Eq + Clone, V: Clone, S: BuildHasher> dbuf::op::Operation<HashTable<(K, V)>, S, ()>
    for HashTableOperation<'_, K, V, S>
{
    fn apply_once(self, buffer: &mut HashTable<(K, V)>, hasher: &S, (): &mut ()) {
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

    fn apply(&mut self, buffer: &mut HashTable<(K, V)>, hasher: &S, (): &mut ()) {
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
