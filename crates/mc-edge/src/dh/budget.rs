//! Server-owned limits; client configuration is never an authorization boundary.
use std::collections::{HashMap, VecDeque};
use std::hash::Hash;
use std::sync::Arc;
use std::time::Instant;

pub(super) const PLAYER_BYTES: usize = 4 * 1024 * 1024;
pub(super) const CACHE_BYTES: usize = 64 * 1024 * 1024;
pub(super) const FRAGMENT: usize = 32 * 1024;
/// One fully framed fragment must fit even when Minecraft compression expands
/// already-compressed DTO bytes; this does not increase the refill rate.
pub(super) const FRAME_BURST: usize = 2 * FRAGMENT;
pub(super) const GLOBAL_JOBS: usize = 8;
pub(super) const PLAYER_JOBS: usize = 2;

pub(super) struct Bucket {
    rate: f64,
    capacity: f64,
    credit: f64,
    at: Instant,
}
impl Bucket {
    pub(super) fn new(rate: usize, capacity: usize) -> Self {
        Self {
            rate: rate as f64,
            capacity: capacity as f64,
            credit: capacity as f64,
            at: Instant::now(),
        }
    }
    pub(super) fn take(&mut self, amount: usize, now: Instant) -> bool {
        self.credit = (self.credit
            + now.saturating_duration_since(self.at).as_secs_f64() * self.rate)
            .min(self.capacity);
        self.at = now;
        if amount as f64 > self.credit {
            return false;
        }
        self.credit -= amount as f64;
        true
    }
    pub(super) fn refund(&mut self, amount: usize) {
        self.credit = (self.credit + amount as f64).min(self.capacity);
    }
}

/// FIFO eviction is deliberately simple: byte accounting, not entry count,
/// bounds both sparse and pathological full-column DTOs.
pub(super) struct Cache<K, V> {
    entries: HashMap<K, (Arc<V>, usize)>,
    order: VecDeque<K>,
    bytes: usize,
    limit: usize,
}
impl<K: Copy + Eq + Hash, V> Cache<K, V> {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            entries: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
            limit,
        }
    }
    pub(super) fn get(&self, key: &K) -> Option<Arc<V>> {
        self.entries.get(key).map(|(value, _)| Arc::clone(value))
    }
    pub(super) fn insert(&mut self, key: K, value: Arc<V>, bytes: usize) {
        if bytes > self.limit || self.entries.contains_key(&key) {
            return;
        }
        while self.bytes + bytes > self.limit {
            let Some(old) = self.order.pop_front() else {
                break;
            };
            if let Some((_, size)) = self.entries.remove(&old) {
                self.bytes -= size;
            }
        }
        self.order.push_back(key);
        self.entries.insert(key, (value, bytes));
        self.bytes += bytes;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn request_and_bandwidth_bursts_are_bounded_and_refill() {
        let mut bucket = Bucket::new(2, PLAYER_JOBS);
        let start = Instant::now();
        assert!(bucket.take(2, start));
        assert!(!bucket.take(1, start));
        assert!(!bucket.take(1, start + Duration::from_millis(499)));
        assert!(bucket.take(1, start + Duration::from_millis(500)));
        assert!(!bucket.take(3, start + Duration::from_secs(100)));
        let mut bandwidth = Bucket::new(500_000, FRAGMENT);
        assert!(bandwidth.take(FRAGMENT, start));
        assert!(!bandwidth.take(1, start));
        bandwidth.refund(FRAGMENT);
        assert!(bandwidth.take(FRAGMENT, start));
    }
    #[test]
    fn cache_is_byte_bounded_and_does_not_duplicate_keys() {
        let mut cache = Cache::new(10);
        cache.insert(1, Arc::new(1), 6);
        cache.insert(1, Arc::new(2), 6);
        assert_eq!(*cache.get(&1).unwrap(), 1);
        cache.insert(2, Arc::new(2), 5);
        assert!(cache.get(&1).is_none());
        assert_eq!(cache.bytes, 5);
        cache.insert(3, Arc::new(3), 11);
        assert!(cache.get(&3).is_none());
        assert_eq!(cache.bytes, 5);
    }
}
