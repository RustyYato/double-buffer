#![forbid(unsafe_code)]

use std::{
    borrow::Borrow,
    hash::{BuildHasher, Hash, RandomState},
    ops::Deref,
};

use hashbrown::HashTable;

#[allow(clippy::type_complexity)]
type TablePointer<T, S> = dbuf::triomphe::OffsetArc<
    dbuf::raw::DoubleBufferData<
        HashTable<T>,
        dbuf::strategy::flashmap::FlashStrategy<dbuf::strategy::park_token::AdaptiveParkToken>,
        S,
    >,
>;

#[allow(clippy::type_complexity)]
pub struct Writer<'env, K, V, S = RandomState> {
    writer: dbuf::op::OpWriter<TablePointer<(K, V), S>, HashTableOperation<'env, K, V, S>>,
}

pub struct Reader<K, V, S> {
    reader: dbuf::raw::Reader<TablePointer<(K, V), S>>,
}

#[allow(clippy::type_complexity)]
pub struct TableGuard<'a, K, V, S> {
    reader: dbuf::raw::ReaderGuard<'a, HashTable<(K, V)>, TablePointer<(K, V), S>>,
}

pub struct ReadGuard<'a, T: ?Sized, K, V, S> {
    reader: dbuf::raw::ReaderGuard<'a, T, TablePointer<(K, V), S>>,
}

impl<T: ?Sized, K, V, S> Deref for ReadGuard<'_, T, K, V, S> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.reader
    }
}

pub enum HashTableOperation<'env, K, V, S> {
    Insert {
        key: K,
        value: V,
    },
    Remove {
        key: K,
    },
    #[allow(clippy::type_complexity)]
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
                dbuf::triomphe::UniqueArc::new(dbuf::raw::DoubleBufferData::with_extras(
                    HashTable::new(),
                    HashTable::new(),
                    dbuf::strategy::flashmap::FlashStrategy::new(),
                    hasher,
                )),
            )),
        }
    }

    pub fn reader(&self) -> Reader<K, V, S> {
        Reader {
            reader: self.writer.reader(),
        }
    }
}

impl<'env, K, V, S: BuildHasher> Writer<'env, K, V, S> {
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
    {
        self.writer.swap_buffers(&mut ());
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

impl<'a, K, V, S> TableGuard<'a, K, V, S> {
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter {
            raw: self.reader.iter(),
        }
    }
}

impl<'a, T: ?Sized, K, V, S> ReadGuard<'a, T, K, V, S> {}

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

impl<K, V, S: Default> Default for Writer<'_, K, V, S> {
    fn default() -> Self {
        Self::with_hasher(Default::default())
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
