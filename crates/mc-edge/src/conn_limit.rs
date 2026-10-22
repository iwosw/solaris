//! Per-address accept limits: a connection throttle and a live-connection cap.
//!
//! Both are operator settings under `[network]` and both default to disabled,
//! so an existing deployment keeps its behaviour. A refused connection is closed
//! immediately, before the handshake reads a byte: the client sees the same
//! "connection reset" a vanilla-style throttle produces, and no per-connection
//! task or buffer is allocated for it.
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// Addresses remembered for throttling before stale ones are pruned.
const MAX_TRACKED_ADDRESSES: usize = 4096;

#[derive(Debug)]
struct AddressEntry {
    last_admitted: Instant,
    live: u32,
}

/// Accept limits for one listener.
#[derive(Debug)]
pub(crate) struct AddressLimiter {
    throttle: Duration,
    maximum: u32,
    addresses: parking_lot::Mutex<HashMap<IpAddr, AddressEntry>>,
}

impl AddressLimiter {
    /// `throttle_ms` is the minimum gap between two accepted connections from one
    /// address (0 disables it); `maximum` is the concurrent cap (0 disables it).
    pub(crate) fn new(throttle_ms: u64, maximum: u32) -> Self {
        Self {
            throttle: Duration::from_millis(throttle_ms),
            maximum,
            addresses: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// Whether a connection from `address` may start now.
    ///
    /// An admitted connection is counted until [`Self::release`], so callers must
    /// pair every `true` with one release after the connection task ends.
    pub(crate) fn admit(&self, address: IpAddr) -> bool {
        let now = Instant::now();
        let mut addresses = self.addresses.lock();
        if addresses.len() > MAX_TRACKED_ADDRESSES {
            // Keep live addresses and addresses still inside their throttle
            // window; the rest can no longer change any decision.
            let throttle = self.throttle;
            addresses.retain(|_, entry| {
                entry.live > 0 || now.duration_since(entry.last_admitted) < throttle
            });
        }
        let entry = addresses.entry(address).or_insert(AddressEntry {
            // A first connection is never throttled.
            last_admitted: now.checked_sub(self.throttle).unwrap_or(now),
            live: 0,
        });
        if !self.throttle.is_zero() && now.duration_since(entry.last_admitted) < self.throttle {
            return false;
        }
        if self.maximum > 0 && entry.live >= self.maximum {
            return false;
        }
        entry.last_admitted = now;
        entry.live = entry.live.saturating_add(1);
        true
    }

    /// Drop one live connection from `address`.
    ///
    /// The throttle stamp survives the last live connection: the gap is measured
    /// between *accepted* connections, so forgetting it here would admit the
    /// reconnect the throttle exists to refuse. Without a throttle there is
    /// nothing to remember, so the entry is dropped immediately.
    pub(crate) fn release(&self, address: IpAddr) {
        let mut addresses = self.addresses.lock();
        let Some(entry) = addresses.get_mut(&address) else {
            return;
        };
        entry.live = entry.live.saturating_sub(1);
        if entry.live == 0 && self.throttle.is_zero() {
            addresses.remove(&address);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip() -> IpAddr {
        "203.0.113.7".parse().unwrap()
    }

    #[test]
    fn disabled_limits_admit_everything() {
        let limiter = AddressLimiter::new(0, 0);
        for _ in 0..64 {
            assert!(limiter.admit(ip()));
        }
    }

    #[test]
    fn throttle_refuses_a_second_connection_and_other_addresses_are_independent() {
        let limiter = AddressLimiter::new(250, 0);
        assert!(limiter.admit(ip()));
        assert!(!limiter.admit(ip()));
        assert!(limiter.admit("203.0.113.8".parse().unwrap()));
        std::thread::sleep(Duration::from_millis(260));
        assert!(limiter.admit(ip()));
    }

    #[test]
    fn the_throttle_stamp_survives_the_last_live_connection() {
        // Regression: releasing the first connection must not forget when it was
        // admitted, or the very reconnect the throttle exists to refuse is let in.
        let limiter = AddressLimiter::new(5_000, 0);
        assert!(limiter.admit(ip()));
        limiter.release(ip());
        assert!(!limiter.admit(ip()));
        std::thread::sleep(Duration::from_millis(5_100));
        assert!(limiter.admit(ip()));
    }

    #[test]
    fn concurrent_cap_counts_live_connections_until_release() {
        let limiter = AddressLimiter::new(0, 2);
        assert!(limiter.admit(ip()));
        assert!(limiter.admit(ip()));
        assert!(!limiter.admit(ip()));
        limiter.release(ip());
        assert!(limiter.admit(ip()));
        limiter.release(ip());
        limiter.release(ip());
        // Releasing more than admitted never underflows, and the idle address
        // is forgotten so the throttle does not outlive the connections.
        limiter.release(ip());
        assert!(limiter.addresses.lock().get(&ip()).is_none());
        assert!(limiter.admit(ip()));
    }
}
