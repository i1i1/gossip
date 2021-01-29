use expiring_map::ExpiringMap;
use tokio::sync::Mutex;

use std::cmp::Eq;
use std::hash::Hash;
use std::time::Duration;

/// Thread safe expiring set. Internally uses Mutex
pub struct ExpiringSet<K>(Mutex<ExpiringMap<K, ()>>);

impl<K: Eq + Hash> ExpiringSet<K> {
    pub fn new(ttl: Duration) -> Self {
        Self(Mutex::new(ExpiringMap::new(ttl)))
    }

    /// Returns true if wan't present
    pub async fn insert(&self, k: K) -> bool {
        self.0.lock().await.insert(k, ()).is_some()
    }

    pub async fn contains(&self, k: &K) -> bool {
        self.0.lock().await.get(k).is_some()
    }
}
