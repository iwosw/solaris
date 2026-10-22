//! Velocity modern player-info forwarding.
//!
//! The proxy opens a backend login, answers a Login Plugin Request on
//! `velocity:player_info` with an HMAC-SHA256-signed payload, and the backend
//! trusts that payload only when the signature matches the shared secret:
//!
//! ```text
//! S → C  LoginPluginRequest   channel=velocity:player_info, data=[4]
//! C → S  LoginPluginResponse  successful, data = HMAC-SHA256(secret, payload) ‖ payload
//! payload = VarInt version, address, UUID, name, properties[, key for v2/v3]
//! ```
//!
//! Wire format and version ceiling follow Velocity's
//! `PlayerDataForwarding.createForwardingData` (MODERN_LAZY_SESSION = 4 for
//! 1.19.3+ clients, which forwards no chat key).

use std::net::IpAddr;

use mc_protocol::codec::ReadMc;
use mc_protocol::packets::login::{GameProfileProperty, MAX_NAME_LEN};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Login plugin channel Velocity answers in modern forwarding mode.
pub const CHANNEL: &str = "velocity:player_info";

/// Highest forwarding layout this server understands (1.19.3+ sends no key).
pub const MAX_FORWARDING_VERSION: u8 = 4;

/// `MODERN_DEFAULT`: UUID, name and properties only.
const MODERN_DEFAULT: i32 = 1;
/// `MODERN_WITH_KEY_V2`: adds the chat key and the signer UUID.
const MODERN_WITH_KEY_V2: i32 = 3;

const SIGNATURE_BYTES: usize = 32;
const MAX_ADDRESS_LEN: usize = 255;
const MAX_PROPERTIES: usize = 16;
const MAX_PROPERTY_NAME_LEN: usize = 64;
const MAX_PROPERTY_VALUE_LEN: usize = 32_767;
const MAX_PROPERTY_SIGNATURE_LEN: usize = 1024;
const MAX_CHAT_KEY_BYTES: usize = 1024;
const MAX_CHAT_KEY_SIGNATURE_BYTES: usize = 1024;

/// One verified forwarded profile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardedPlayer {
    /// Player address as the proxy saw it (dotted IPv4 or IPv6 text).
    pub remote_address: String,
    pub uuid: Uuid,
    pub name: String,
    pub properties: Vec<GameProfileProperty>,
}

impl ForwardedPlayer {
    /// Parsed client address; unparsable proxy text does not admit a login.
    #[must_use]
    pub fn ip(&self) -> Option<IpAddr> {
        self.remote_address.parse().ok()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ForwardingError {
    #[error("proxy.secret must not be empty in velocity mode")]
    EmptySecret,
    #[error("forwarding payload is shorter than its HMAC signature")]
    Truncated,
    #[error("forwarding signature does not match the configured secret")]
    SignatureMismatch,
    #[error("unsupported forwarding version {0}")]
    UnsupportedVersion(i32),
    #[error("malformed forwarding payload: {0}")]
    Malformed(#[from] mc_protocol::CodecError),
}

/// Verified Velocity modern-forwarding secret.
#[derive(Clone)]
pub struct VelocityForwarding {
    secret: Vec<u8>,
}

impl std::fmt::Debug for VelocityForwarding {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VelocityForwarding")
            .field("secret_bytes", &self.secret.len())
            .finish()
    }
}

impl VelocityForwarding {
    pub fn new(secret: &str) -> Result<Self, ForwardingError> {
        if secret.is_empty() {
            return Err(ForwardingError::EmptySecret);
        }
        Ok(Self {
            secret: secret.as_bytes().to_vec(),
        })
    }

    /// The single byte the backend sends in the plugin request.
    #[must_use]
    pub fn request_version(&self) -> u8 {
        MAX_FORWARDING_VERSION
    }

    /// Verifies the response HMAC and decodes the forwarded profile.
    pub fn verify_and_parse(&self, data: &[u8]) -> Result<ForwardedPlayer, ForwardingError> {
        if data.len() < SIGNATURE_BYTES {
            return Err(ForwardingError::Truncated);
        }
        let (signature, payload) = data.split_at(SIGNATURE_BYTES);
        if !constant_time_eq(&hmac_sha256(&self.secret, payload), signature) {
            return Err(ForwardingError::SignatureMismatch);
        }
        let mut buf = payload;
        let version = buf.read_varint()?;
        if !(MODERN_DEFAULT..=i32::from(MAX_FORWARDING_VERSION)).contains(&version) {
            return Err(ForwardingError::UnsupportedVersion(version));
        }
        let remote_address = buf.read_string(MAX_ADDRESS_LEN)?;
        let uuid = buf.read_uuid()?;
        let name = buf.read_string(MAX_NAME_LEN)?;
        let property_count = buf.read_varint()?;
        if property_count < 0 || property_count as usize > MAX_PROPERTIES {
            return Err(ForwardingError::Malformed(
                mc_protocol::CodecError::StringTooLong {
                    len: property_count.max(0) as usize,
                    max: MAX_PROPERTIES,
                },
            ));
        }
        let mut properties = Vec::with_capacity(property_count as usize);
        for _ in 0..property_count {
            let name = buf.read_string(MAX_PROPERTY_NAME_LEN)?;
            let value = buf.read_string(MAX_PROPERTY_VALUE_LEN)?;
            let signature = if buf.read_bool()? {
                Some(buf.read_string(MAX_PROPERTY_SIGNATURE_LEN)?)
            } else {
                None
            };
            properties.push(GameProfileProperty {
                name,
                value,
                signature,
            });
        }
        // 1.19.3+ (version 4) forwards no chat key; older layouts append one.
        if (2..=MODERN_WITH_KEY_V2).contains(&version) {
            let _expires_at = buf.read_i64()?;
            let _key = buf.read_byte_array(MAX_CHAT_KEY_BYTES)?;
            let _key_signature = buf.read_byte_array(MAX_CHAT_KEY_SIGNATURE_BYTES)?;
            if version == MODERN_WITH_KEY_V2 && buf.read_bool()? {
                let _signer = buf.read_uuid()?;
            }
        }
        Ok(ForwardedPlayer {
            remote_address,
            uuid,
            name,
            properties,
        })
    }
}

/// HMAC-SHA256 (RFC 2104) over one message.
pub(crate) fn hmac_sha256(key: &[u8], message: &[u8]) -> [u8; SIGNATURE_BYTES] {
    const BLOCK_BYTES: usize = 64;
    let mut key_block = [0u8; BLOCK_BYTES];
    if key.len() > BLOCK_BYTES {
        key_block[..SIGNATURE_BYTES].copy_from_slice(&Sha256::digest(key));
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner_pad = [0x36u8; BLOCK_BYTES];
    let mut outer_pad = [0x5cu8; BLOCK_BYTES];
    for (index, byte) in key_block.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    let mut mac = [0u8; SIGNATURE_BYTES];
    mac.copy_from_slice(&outer.finalize());
    mac
}

/// Compares two MACs without an early exit on the first differing byte.
fn constant_time_eq(expected: &[u8], actual: &[u8]) -> bool {
    if expected.len() != actual.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left, right) in expected.iter().zip(actual) {
        difference |= left ^ right;
    }
    difference == 0
}

#[cfg(test)]
#[path = "proxy_tests.rs"]
mod tests;
