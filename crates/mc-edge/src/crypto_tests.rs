//! Tests for [`crate::crypto`]: cipher vectors, offline UUIDs, login hash,
//! and the RSA challenge round-trip.

use rsa::pkcs8::DecodePublicKey;
use rsa::traits::PublicKeyParts;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};

use super::{RsaIdentity, java_signed_hex, minecraft_server_hash, offline_uuid};

#[test]
fn offline_uuid_is_pinned() {
    let cases: &[(&str, &str)] = &[
        ("Notch", "b50ad385-829d-3141-a216-7e7d7539ba7f"),
        ("Player", "a01e3843-e521-3998-958a-f459800e4d11"),
        ("jeb_", "a762f560-4fce-3236-812a-b80efff0b62b"),
        ("Steve", "5627dd98-e6be-3c21-b8a8-e92344183641"),
        ("Dinnerbone", "4d258a81-2358-3084-8166-05b9faccad80"),
    ];
    for (name, expected) in cases {
        assert_eq!(
            offline_uuid(name),
            uuid::Uuid::parse_str(expected).unwrap(),
            "name {name:?}",
        );
    }
}

#[test]
fn offline_uuid_is_version_3_variant_rfc4122() {
    let bytes = *offline_uuid("anyone").as_bytes();
    assert_eq!(bytes[6] >> 4, 3, "version nibble");
    assert_eq!(bytes[8] >> 6, 0b10, "RFC 4122 variant");
}

#[test]
fn offline_uuid_is_deterministic() {
    assert_eq!(offline_uuid("repeat"), offline_uuid("repeat"));
    assert_ne!(offline_uuid("a"), offline_uuid("b"));
}

#[test]
fn java_signed_hex_vectors() {
    assert_eq!(java_signed_hex(&[0x7f, 0]), "7f00");
    assert_eq!(java_signed_hex(&[0x80]), "-80");
    assert_eq!(java_signed_hex(&[0xff]), "-1");
    assert_eq!(java_signed_hex(&[0xff, 0]), "-100");
}

#[test]
fn server_hash_matches_known_vectors() {
    assert_eq!(
        minecraft_server_hash(
            b"",
            &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15],
            b"public-key-der"
        ),
        "5242c3c07c6d11c534151bb8c15f8481019b4a93"
    );
    assert_eq!(
        minecraft_server_hash(b"", &[0xff; 16], b"public-key-der"),
        "-600ab57a13a4f1476a78aaeec78b145aa9d5b960"
    );
}

#[test]
fn cipher_round_trip_is_symmetric() {
    use super::MinecraftCipher;
    let secret = [0x5a; 16];
    let mut enc = MinecraftCipher::new(&secret);
    let mut dec = MinecraftCipher::new(&secret);
    let mut buf = b"hello minecraft".to_vec();
    enc.encrypt_in_place(&mut buf);
    assert_ne!(buf, b"hello minecraft");
    dec.decrypt_in_place(&mut buf);
    assert_eq!(buf, b"hello minecraft");
}

#[test]
fn cipher_streams_are_direction_independent() {
    use super::MinecraftCipher;
    let secret = [0x11; 16];
    let mut a = MinecraftCipher::new(&secret);
    let mut b = MinecraftCipher::new(&secret);
    let mut out = vec![7u8; 32];
    a.encrypt_in_place(&mut out);
    let mut back = out.clone();
    b.decrypt_in_place(&mut back);
    assert_eq!(back, vec![7u8; 32]);
}

#[test]
fn rsa_identity_decrypts_valid_response() {
    let identity = RsaIdentity::generate().expect("generate RSA identity");
    let public_key = RsaPublicKey::from_public_key_der(identity.public_key_der())
        .expect("decode SPKI public key");
    assert_eq!(public_key.size() * 8, 1024);

    let challenge = identity.new_challenge();
    let secret = [0x5a; 16];
    let mut rng = rsa::rand_core::OsRng;
    let encrypted_secret = public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &secret)
        .expect("encrypt secret");
    let encrypted_challenge = public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &challenge)
        .expect("encrypt challenge");

    assert_eq!(
        identity
            .decrypt_response(&encrypted_secret, &encrypted_challenge, challenge)
            .unwrap(),
        secret
    );
}

#[test]
fn rsa_identity_rejects_wrong_challenge() {
    let identity = RsaIdentity::generate().expect("generate RSA identity");
    let public_key = RsaPublicKey::from_public_key_der(identity.public_key_der())
        .expect("decode SPKI public key");
    let mut rng = rsa::rand_core::OsRng;
    let secret = [1u8; 16];
    let encrypted_secret = public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &secret)
        .unwrap();
    let encrypted_challenge = public_key
        .encrypt(&mut rng, Pkcs1v15Encrypt, &[9, 9, 9, 9])
        .unwrap();
    assert!(
        identity
            .decrypt_response(&encrypted_secret, &encrypted_challenge, [1, 2, 3, 4])
            .is_err()
    );
}
