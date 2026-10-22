//! Tests for [`crate::proxy`]: HMAC verification and the Velocity modern
//! forwarding payload layouts.

use mc_protocol::codec::WriteMc;
use mc_protocol::packets::login::GameProfileProperty;
use uuid::Uuid;

use super::{ForwardingError, MAX_FORWARDING_VERSION, VelocityForwarding};

fn sign(secret: &str, payload: &[u8]) -> Vec<u8> {
    let mut signed = super::hmac_sha256(secret.as_bytes(), payload).to_vec();
    signed.extend_from_slice(payload);
    signed
}

fn payload(version: i32, name: &str, uuid: Uuid, with_key: bool) -> Vec<u8> {
    let mut body = Vec::new();
    body.write_varint(version);
    body.write_string("203.0.113.7", 255).unwrap();
    body.write_uuid(uuid);
    body.write_string(name, 16).unwrap();
    body.write_varint(1);
    body.write_string("textures", 32_767).unwrap();
    body.write_string("{\"value\":\"skin\"}", 32_767).unwrap();
    body.write_bool(true);
    body.write_string("signature", 32_767).unwrap();
    if with_key {
        body.write_i64(1_700_000_000_000);
        body.write_byte_array(&[7u8; 64]).unwrap();
        body.write_byte_array(&[8u8; 64]).unwrap();
        if version == 3 {
            body.write_bool(true);
            body.write_uuid(Uuid::from_u128(0xDEAD));
        }
    }
    body
}

fn forwarding() -> VelocityForwarding {
    VelocityForwarding::new("s3cret").unwrap()
}

#[test]
fn hmac_sha256_matches_rfc_4231_case_two() {
    let mac = super::hmac_sha256(b"Jefe", b"what do ya want for nothing?");
    let expected: [u8; 32] = [
        0x5b, 0xdc, 0xc1, 0x46, 0xbf, 0x60, 0x75, 0x4e, 0x6a, 0x04, 0x24, 0x26, 0x08, 0x95, 0x75,
        0xc7, 0x5a, 0x00, 0x3f, 0x08, 0x9d, 0x27, 0x39, 0x83, 0x9d, 0xec, 0x58, 0xb9, 0x64, 0xec,
        0x38, 0x43,
    ];
    assert_eq!(mac, expected);
}

#[test]
fn empty_secret_is_refused() {
    assert!(matches!(
        VelocityForwarding::new(""),
        Err(ForwardingError::EmptySecret)
    ));
}

#[test]
fn version_four_payload_yields_forwarded_profile() {
    let uuid = Uuid::from_u128(0x0123_4567_89ab_cdef_fedc_ba98_7654_3210);
    let data = sign(
        "s3cret",
        &payload(i32::from(MAX_FORWARDING_VERSION), "Player_1", uuid, false),
    );
    let forwarded = forwarding().verify_and_parse(&data).unwrap();
    assert_eq!(forwarded.uuid, uuid);
    assert_eq!(forwarded.name, "Player_1");
    assert_eq!(forwarded.remote_address, "203.0.113.7");
    assert_eq!(forwarded.ip().unwrap().to_string(), "203.0.113.7");
    assert_eq!(
        forwarded.properties,
        vec![GameProfileProperty {
            name: "textures".into(),
            value: "{\"value\":\"skin\"}".into(),
            signature: Some("signature".into()),
        }]
    );
}

#[test]
fn chat_key_layouts_of_versions_two_and_three_are_parsed() {
    let uuid = Uuid::from_u128(9);
    for version in [2, 3] {
        let data = sign("s3cret", &payload(version, "Keyed", uuid, true));
        let forwarded = forwarding().verify_and_parse(&data).unwrap();
        assert_eq!(forwarded.uuid, uuid);
        assert_eq!(forwarded.name, "Keyed");
    }
}

#[test]
fn wrong_secret_is_rejected() {
    let data = sign(
        "other",
        &payload(
            i32::from(MAX_FORWARDING_VERSION),
            "Player",
            Uuid::nil(),
            false,
        ),
    );
    assert!(matches!(
        forwarding().verify_and_parse(&data),
        Err(ForwardingError::SignatureMismatch)
    ));
}

#[test]
fn tampered_payload_is_rejected() {
    let mut data = sign(
        "s3cret",
        &payload(
            i32::from(MAX_FORWARDING_VERSION),
            "Player",
            Uuid::nil(),
            false,
        ),
    );
    let last = data.len() - 1;
    data[last] ^= 0x01;
    assert!(matches!(
        forwarding().verify_and_parse(&data),
        Err(ForwardingError::SignatureMismatch)
    ));
}

#[test]
fn truncated_payload_is_rejected() {
    assert!(matches!(
        forwarding().verify_and_parse(&[0u8; 31]),
        Err(ForwardingError::Truncated)
    ));
}

#[test]
fn unsupported_version_is_rejected_even_when_signed() {
    let data = sign("s3cret", &payload(5, "Player", Uuid::nil(), false));
    assert!(matches!(
        forwarding().verify_and_parse(&data),
        Err(ForwardingError::UnsupportedVersion(5))
    ));
    let zero = sign("s3cret", &payload(0, "Player", Uuid::nil(), false));
    assert!(matches!(
        forwarding().verify_and_parse(&zero),
        Err(ForwardingError::UnsupportedVersion(0))
    ));
}

#[test]
fn malformed_payload_is_rejected_after_a_valid_signature() {
    let mut body = Vec::new();
    body.write_varint(4);
    body.write_string("203.0.113.7", 255).unwrap();
    let data = sign("s3cret", &body);
    assert!(matches!(
        forwarding().verify_and_parse(&data),
        Err(ForwardingError::Malformed(_))
    ));
}
