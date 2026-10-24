use super::*;
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

#[test]
fn accept_exhaustion_waits_for_a_session_instead_of_stopping_the_server() {
    for errno in [23, 24, 105, 12] {
        let error = std::io::Error::from_raw_os_error(errno);
        assert_eq!(
            accept_failure(&error, 3),
            AcceptFailure::WaitForSession,
            "errno {errno}"
        );
        // With no session of ours to release a socket, waiting would hang:
        // fail with a diagnostic instead.
        assert_eq!(accept_failure(&error, 0), AcceptFailure::Fatal);
    }
    let aborted = std::io::Error::from(std::io::ErrorKind::ConnectionAborted);
    assert_eq!(accept_failure(&aborted, 0), AcceptFailure::Transient);
    let broken = std::io::Error::from_raw_os_error(22); // EINVAL: listener broke
    assert_eq!(accept_failure(&broken, 3), AcceptFailure::Fatal);
}

#[test]
fn screen_sharing_admits_native_and_fabric_loader_sessions() {
    use mc_loader::LoaderPlatform;

    assert!(supports_screen_sharing(Some(LoaderPlatform::Native)));
    assert!(supports_screen_sharing(Some(LoaderPlatform::Fabric)));
    assert!(!supports_screen_sharing(None));
    assert!(!supports_screen_sharing(Some(LoaderPlatform::Forge)));
    assert!(!supports_screen_sharing(Some(LoaderPlatform::NeoForge)));
}
