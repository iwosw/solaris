use super::*;
use std::cell::Cell;
use std::time::Duration;

#[test]
fn rotating_unknown_ids_share_one_bounded_bucket() {
    let config = crate::config::AntiCheatSection {
        packet_rate_per_second: 2,
        packet_burst: 2,
        max_airtime_ticks: 80,
    };
    let mut limiter = PacketLimiter::new(&config);
    let now = Instant::now();
    assert!(limiter.allow_at(65_536, now));
    assert!(limiter.allow_at(65_537, now));
    assert!(!limiter.allow_at(65_538, now));
    for id in [-1, i32::MIN, i32::MAX].into_iter().chain(65_539..66_539) {
        assert!(!limiter.allow_at(id, now), "fresh bucket for {id}");
    }
    assert_eq!(limiter.buckets.len(), 1);
    assert!(limiter.allow_at(-1, now + Duration::from_millis(500)));
    assert!(
        limiter.allow_at(5, now),
        "known traffic remains independent"
    );
}

#[test]
fn every_packet_id_has_bounded_limiter_storage() {
    let mut limiter = PacketLimiter::new(&crate::config::AntiCheatSection::default());
    let now = Instant::now();
    for id in -1_024..1_024 {
        let _ = limiter.allow_at(id, now);
    }
    assert!(limiter.buckets.len() <= 70);
}

#[tokio::test(start_paused = true)]
async fn socket_exhaustion_retries_with_backoff_then_accepts() {
    // Linux ENFILE and EMFILE, followed by a transient aborted connection.
    let mut results = [
        Err(std::io::Error::from_raw_os_error(23)),
        Err(std::io::Error::from_raw_os_error(24)),
        Err(std::io::Error::from(std::io::ErrorKind::ConnectionAborted)),
        Ok(42),
    ]
    .into_iter();
    let start = tokio::time::Instant::now();
    let mut attempts = Vec::new();
    let mut retry_at = None;
    let accepted = accept_with_backoff(
        || {
            attempts.push(tokio::time::Instant::now());
            std::future::ready(results.next().expect("unexpected accept attempt"))
        },
        &mut retry_at,
    )
    .await;
    assert_eq!(accepted, 42);
    assert_eq!(attempts.len(), 4);
    assert_eq!(attempts[0], start);
    for pair in attempts.windows(2) {
        assert_eq!(pair[1] - pair[0], Duration::from_millis(100));
    }
    assert!(retry_at.is_none());
}

#[tokio::test(start_paused = true)]
async fn accept_backoff_survives_select_cancellation() {
    let attempts = Cell::new(0);
    let start = tokio::time::Instant::now();
    let mut retry_at = None;
    {
        let attempt = accept_with_backoff(
            || {
                attempts.set(attempts.get() + 1);
                std::future::ready(Err::<(), _>(std::io::Error::from_raw_os_error(24)))
            },
            &mut retry_at,
        );
        tokio::pin!(attempt);
        assert!(
            std::future::poll_fn(|cx| std::task::Poll::Ready(attempt.as_mut().poll(cx)))
                .await
                .is_pending()
        );
    }
    assert_eq!(attempts.get(), 1);
    assert_eq!(retry_at, Some(start + Duration::from_millis(100)));
    let accepted = accept_with_backoff(
        || {
            attempts.set(attempts.get() + 1);
            std::future::ready(Ok(7))
        },
        &mut retry_at,
    )
    .await;
    assert_eq!(accepted, 7);
    assert_eq!(attempts.get(), 2);
    assert_eq!(
        tokio::time::Instant::now() - start,
        Duration::from_millis(100)
    );
}
