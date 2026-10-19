//! Native password work stays on the plugin worker, never in guest fuel or tick.
use argon2::Argon2;
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use rand_core::OsRng;

const MAX_PASSWORD_BYTES: usize = 256;

pub(crate) fn hash(password: &str) -> wasmtime::Result<String> {
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(wasmtime::Error::msg("password exceeds 256 bytes"));
    }
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|_| wasmtime::Error::msg("native password hashing failed"))
}

pub(crate) fn verify(password: &str, phc: &str) -> bool {
    if password.len() > MAX_PASSWORD_BYTES {
        return false;
    }
    PasswordHash::new(phc).is_ok_and(|parsed| {
        let argon = Argon2::default();
        let expected = argon.params();
        // PHC parameters come from a guest. Bound native work before Argon2
        // allocates memory or runs: guest fuel/store limits cannot cover it.
        parsed.algorithm.as_str() == "argon2id"
            && parsed.version == Some(0x13)
            && parsed.params.iter().count() == 3
            && parsed.params.get_decimal("m") == Some(expected.m_cost())
            && parsed.params.get_decimal("t") == Some(expected.t_cost())
            && parsed.params.get_decimal("p") == Some(expected.p_cost())
            && argon.verify_password(password.as_bytes(), &parsed).is_ok()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_hash_roundtrip_uses_argon2id_and_random_salts() {
        let started = std::time::Instant::now();
        let first = hash("correct horse battery staple").unwrap();
        eprintln!(
            "native Argon2id one-hash elapsed_ms={:.3}",
            started.elapsed().as_secs_f64() * 1000.0
        );
        let change_started = std::time::Instant::now();
        assert!(verify("correct horse battery staple", &first));
        let second = hash("correct horse battery staple").unwrap();
        eprintln!(
            "native Argon2id verify-plus-hash elapsed_ms={:.3}",
            change_started.elapsed().as_secs_f64() * 1000.0
        );
        assert!(first.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"));
        assert_ne!(first, second);
        assert!(!verify("wrong password", &first));
    }

    #[test]
    fn password_verify_malformed_phc_fails_closed() {
        for phc in [
            "",
            "not a PHC",
            "$argon2id$",
            "$argon2id$v=19$m=nope,t=2,p=1$salt$hash",
        ] {
            assert!(!verify("password", phc));
        }
    }

    #[test]
    fn password_verify_rejects_unbounded_or_foreign_phc_parameters() {
        for header in [
            "argon2id$v=19$m=4294967295,t=2,p=1",
            "argon2id$v=19$m=19456,t=4294967295,p=1",
            "argon2i$v=19$m=19456,t=2,p=1",
            "argon2id$v=16$m=19456,t=2,p=1",
            "argon2id$v=19$m=19456,t=2,p=2",
        ] {
            let phc = format!("${header}$c29tZXNhbHQ$MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY");
            let parsed = PasswordHash::new(&phc);
            assert!(
                parsed.is_ok(),
                "valid synthetic PHC {header}: {:?}",
                parsed.err()
            );
            assert!(!verify("password", &phc));
        }
    }

    #[test]
    fn password_host_rejects_oversized_passwords_before_native_work() {
        let oversized = "x".repeat(MAX_PASSWORD_BYTES + 1);
        assert!(hash(&oversized).is_err());
        assert!(!verify(&oversized, "invalid"));
    }

    #[test]
    fn password_host_imports_are_refused_in_precommit_hooks() {
        use crate::bindings::solaris::plugin::host::Host;
        struct Services;
        impl crate::HostServices for Services {
            fn log(&mut self, _: crate::LogLevel, _: &str) {
                panic!("password imports must not log");
            }
            fn plugin_id(&self) -> &str {
                "auth"
            }
        }
        let mut state = crate::InstanceState::new(Services, &crate::PluginLimits::default());
        state.enter_hook_phase();
        assert!(
            Host::password_hash(&mut state, "secret".into())
                .unwrap()
                .is_empty()
        );
        assert!(!Host::password_verify(
            &mut state,
            "secret".into(),
            "invalid".into()
        ));
        assert_eq!(state.leave_hook_phase(), 2);
    }
}
