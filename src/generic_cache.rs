use std::fmt::Display;
use std::hash::Hash;

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{OwnedRwLockWriteGuard, RwLock};

pub struct GenericCache<TKey, TValue> {
    cached_data: DashMap<TKey, (Arc<TValue>, Instant)>,
    active_writes: DashMap<TKey, Arc<RwLock<()>>>,
    timeout: Duration,
}

impl<TKey: Hash + Eq + Display + Clone, TValue> GenericCache<TKey, TValue> {
    /// Creates a new `GenericCache`
    pub fn new() -> GenericCache<TKey, TValue> {
        GenericCache {
            cached_data: DashMap::new(),
            active_writes: DashMap::new(),
            timeout: Duration::from_secs(60), // TODO: 30 minutes (60s is useful for testing)
        }
    }

    /// Gets the cached data if available, waiting for it if there is an active writer (to avoid
    /// double work). If the data is not available and there is no other task busy with writing it,
    /// returns not found.
    pub async fn get_cached(&self, key: &TKey) -> GetCachedResult<TValue> {
        loop {
            if let Some(repodata) = self.cached_data.get(key) {
                if Instant::now() > repodata.value().1 + self.timeout {
                    println!("Cache hit, but data was stale: {key}");
                } else {
                    println!("Cache hit: {key}");
                    return GetCachedResult::Found(repodata.value().0.clone());
                }
            }

            // Cache miss
            match self.active_writes.entry(key.clone()) {
                Entry::Occupied(e) => {
                    // A download is going on. Wait for it to finish and try to get the result in
                    // the next loop iteration
                    println!("Download already started, waiting for it to finish...");
                    let _ = e.get().read().await;
                    println!("Finished, continuing...");

                    // The write is no longer active (it is crucial to drop the entry first to avoid
                    // deadlocks)
                    drop(e);
                    self.active_writes.remove(&key);
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
