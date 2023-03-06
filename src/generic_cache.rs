use std::fmt::Display;
use std::hash::Hash;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};
use tracing::{event, Level};

#[cfg(test)]
use mock_instant::Instant;

#[cfg(not(test))]
use std::time::Instant;

pub struct GenericCache<TKey, TValue> {
    cached_data: DashMap<TKey, (Arc<TValue>, Instant)>,
    active_writes: DashMap<TKey, Arc<RwLock<()>>>,
    expiration: Duration,
}

impl<TKey: Hash + Eq + Display + Clone, TValue> GenericCache<TKey, TValue> {
    /// Creates a new `GenericCache`
    pub fn with_expiration(expiration: Duration) -> GenericCache<TKey, TValue> {
        GenericCache {
            cached_data: DashMap::new(),
            active_writes: DashMap::new(),
            expiration,
        }
    }

    /// Removes outdated data from the cache
    pub fn gc(&self) {
        let mut expired_keys = Vec::new();
        for item in &self.cached_data {
            let key = item.key();
            let (_value, insert_instant) = item.value();
            if Instant::now() > *insert_instant + self.expiration {
                event!(Level::TRACE, "Key marked for GC: {key}");

                // We remove the keys in a separate step to avoid deadlocks
                expired_keys.push(key.clone());
            }
        }

        for key in &expired_keys {
            self.cached_data.remove(key);
        }

        event!(
            Level::DEBUG,
            "GC cleared {} keys from cache",
            expired_keys.len()
        );
    }

    /// Gets the cached data if available, waiting for it if there is an active writer (to avoid
    /// double work). If the data is not available and there is no other task busy with writing it,
    /// returns not found.
    pub async fn get_cached(&self, key: &TKey) -> GetCachedResult<TValue> {
        loop {
            if let Some(repodata) = self.cached_data.get(key) {
                if Instant::now() > repodata.value().1 + self.expiration {
                    event!(Level::TRACE, "Cache hit, but data was stale: {key}");
                } else {
                    event!(Level::TRACE, "Cache hit: {key}");
                    return GetCachedResult::Found(repodata.value().0.clone());
                }
            }

            // Cache miss
            match self.active_writes.entry(key.clone()) {
                Entry::Occupied(e) => {
                    // A download is going on. Wait for it to finish and try to get the result in
                    // the next loop iteration
                    event!(
                        Level::TRACE,
                        "Download already started, waiting for it to finish..."
                    );
                    let _ = e.get().read().await;
                }
                Entry::Vacant(e) => {
                    // No download is going on, register ours so others can see it (there can still
                    // be races here, making it in theory possible to have parallel downloads of the
                    // same repodata.json, but we are ok with that)
                    let lock = Arc::new(RwLock::new(()));
                    let write_guard = lock.clone().write_owned().await;
                    e.insert(lock);
                    return GetCachedResult::NotFound(write_guard);
                }
            };
        }
    }

    /// Caches the value at the given key and notifies
    pub fn set(&self, key: TKey, value: Arc<TValue>, guard: OwnedRwLockWriteGuard<()>) {
        self.cached_data
            .insert(key.clone(), (value, Instant::now()));

        // This will notify anyone who is waiting for the write to finish
        drop(guard);

        // Remove the active write, since it is no longer necessary
        self.active_writes.remove(&key);
    }
}

/// Represents the result of a call to [`GenericCache::get_cached`]
pub enum GetCachedResult<T> {
    /// The key was found in the cache and its value is included in the enum variant
    Found(Arc<T>),
    /// The key was not found in the cache and there are no active writes, so the caller is expected
    /// to retrieve the value from somewhere else and write it to the cache by calling
    /// [`GenericCache::set`] with the provided write guard
    NotFound(OwnedRwLockWriteGuard<()>),
}

#[cfg(test)]
mod test {
    use super::*;
    use mock_instant::MockClock;

    #[tokio::test]
    async fn test_gc_works() {
        let cache = GenericCache::with_expiration(Duration::from_secs(60));
        add_item(&cache, 42, "foo").await;

        // Sanity check
        assert_eq!(cache.cached_data.len(), 1);

        // No time has passed, GC does not collect anything
        cache.gc();
        assert_eq!(cache.cached_data.len(), 1);

        // Additional item inserted after 30 seconds
        MockClock::advance(Duration::from_secs(30));
        add_item(&cache, 43, "bar").await;

        // Sanity check
        assert_eq!(cache.cached_data.len(), 2);

        // More than a minute has passed, GC collects the first item, but not the second
        MockClock::advance(Duration::from_secs(40));
        cache.gc();
        assert_eq!(cache.cached_data.len(), 1);
        let (key, value) = cache.cached_data.into_iter().next().unwrap();
        assert_eq!(key, 43);
        assert_eq!(*value.0.as_ref(), "bar");
    }

    async fn add_item(cache: &GenericCache<usize, &'static str>, key: usize, value: &'static str) {
        match cache.get_cached(&key).await {
            GetCachedResult::Found(_) => unreachable!(),
            GetCachedResult::NotFound(rw_guard) => {
                cache.set(key, Arc::new(value), rw_guard);
            }
        }
    }
}
