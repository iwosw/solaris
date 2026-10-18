//! Transport crypto: AES-128-CFB8 stream cipher, offline UUIDs, Mojang auth.
//!
//! Handles encrypted connections and session authentication. Packet IDs and
//! wire layouts come only from `mc-protocol`.

use std::fmt;
use std::future::Future;
use std::net::IpAddr;
use std::pin::Pin;
use std::time::Duration;

use aes::{Aes128, cipher::KeyIvInit};
use cfb8::{Decryptor, Encryptor};
use mc_protocol::packets::login::GameProfileProperty;
use md5::{Digest, Md5};
use num_bigint::BigInt;
use rsa::pkcs8::EncodePublicKey;
use rsa::rand_core::{OsRng, RngCore};
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use sha1::Sha1;
// ---------------------------------------------------------------------------
// AES-128-CFB8 transport cipher
// ---------------------------------------------------------------------------

/// Stateful AES-128-CFB8 cipher pair used by the Minecraft transport.
///
/// Minecraft uses the shared secret as both the AES key and initialization
/// vector. Encryption and decryption are independent byte streams, so each
/// direction keeps its own feedback state.
pub struct MinecraftCipher {
    encryptor: Encryptor<Aes128>,
    decryptor: Decryptor<Aes128>,
}

impl MinecraftCipher {
    /// Creates fresh encryption and decryption streams from a shared secret.
    #[must_use]
    pub fn new(shared_secret: &[u8; 16]) -> Self {
        Self {
            encryptor: Encryptor::new_from_slices(shared_secret, shared_secret)
                .expect("16-byte key and IV are valid for AES-128"),
            decryptor: Decryptor::new_from_slices(shared_secret, shared_secret)
                .expect("16-byte key and IV are valid for AES-128"),
        }
    }

    /// Encrypts the next bytes in the outbound stream.
    pub fn encrypt_in_place(&mut self, bytes: &mut [u8]) {
        self.encryptor.encrypt(bytes);
    }

    /// Decrypts the next bytes in the inbound stream.
    pub fn decrypt_in_place(&mut self, bytes: &mut [u8]) {
        self.decryptor.decrypt(bytes);
    }
}

// ---------------------------------------------------------------------------
// Offline UUIDs
// ---------------------------------------------------------------------------

/// Java's `UUID.nameUUIDFromBytes("OfflinePlayer:" + name)`, byte-for-byte.
///
/// This is the de-facto standard offline-mode UUID derivation. Reproducing
/// it exactly means a player who connects to a vanilla offline server and
/// later to a Solaris offline server keeps the same UUID, so any external
/// persistence (whitelists, permissions, claims) survives the migration.
#[must_use]
pub fn offline_uuid(name: &str) -> uuid::Uuid {
    let mut hasher = Md5::new();
    hasher.update(b"OfflinePlayer:");
    hasher.update(name.as_bytes());
    let mut bytes: [u8; 16] = hasher.finalize().into();
    // Set version 3 (name-based MD5) and the RFC 4122 variant, matching
    // `UUID.nameUUIDFromBytes`.
    bytes[6] = (bytes[6] & 0x0F) | 0x30;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
    uuid::Uuid::from_bytes(bytes)
}

// ---------------------------------------------------------------------------
// RSA login identity
// ---------------------------------------------------------------------------

const RSA_KEY_BITS: usize = 1024;

const MOJANG_HAS_JOINED_URL: &str = "https://sessionserver.mojang.com/session/minecraft/hasJoined";
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_HAS_JOINED_BODY_BYTES: usize = 64 * 1024;
const MAX_PROFILE_PROPERTIES: usize = 16;
const MAX_PROPERTY_NAME_BYTES: usize = 64;
const MAX_PROPERTY_VALUE_BYTES: usize = 16 * 1024;
const MAX_PROPERTY_SIGNATURE_BYTES: usize = 16 * 1024;

/// RSA identity used by the Minecraft login encryption handshake.
pub struct RsaIdentity {
    private_key: RsaPrivateKey,
    public_key_der: Vec<u8>,
}

impl fmt::Debug for RsaIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RsaIdentity")
            .field("public_key_der_len", &self.public_key_der.len())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum RsaIdentityError {
    #[error("RSA operation failed")]
    Crypto,
    #[error("encrypted login response contained the wrong challenge")]
    InvalidChallenge,
    #[error("shared secret must contain exactly 16 bytes, got {0}")]
    InvalidSharedSecretLength(usize),
}

impl RsaIdentity {
    pub fn generate() -> Result<Self, RsaIdentityError> {
        let private_key =
            RsaPrivateKey::new(&mut OsRng, RSA_KEY_BITS).map_err(|_| RsaIdentityError::Crypto)?;
        let public_key_der = RsaPublicKey::from(&private_key)
            .to_public_key_der()
            .map_err(|_| RsaIdentityError::Crypto)?
            .as_bytes()
            .to_vec();
        Ok(Self {
            private_key,
            public_key_der,
        })
    }

    #[must_use]
    pub fn public_key_der(&self) -> &[u8] {
        &self.public_key_der
    }

    #[must_use]
    pub fn new_challenge(&self) -> [u8; 4] {
        let mut challenge = [0; 4];
        OsRng.fill_bytes(&mut challenge);
        challenge
    }

    pub fn decrypt_response(
        &self,
        encrypted_secret: &[u8],
        encrypted_challenge: &[u8],
        expected_challenge: [u8; 4],
    ) -> Result<[u8; 16], RsaIdentityError> {
        let challenge = self
            .private_key
            .decrypt(Pkcs1v15Encrypt, encrypted_challenge)
            .map_err(|_| RsaIdentityError::Crypto)?;
        if challenge != expected_challenge {
            return Err(RsaIdentityError::InvalidChallenge);
        }

        let secret = self
            .private_key
            .decrypt(Pkcs1v15Encrypt, encrypted_secret)
            .map_err(|_| RsaIdentityError::Crypto)?;
        let secret_len = secret.len();
        secret
            .try_into()
            .map_err(|_| RsaIdentityError::InvalidSharedSecretLength(secret_len))
    }
}

/// Computes the vanilla login hash. `server_id` contains ISO-8859-1 bytes.
#[must_use]
pub fn minecraft_server_hash(
    server_id: &[u8],
    shared_secret: &[u8; 16],
    public_key_der: &[u8],
) -> String {
    let mut digest = Sha1::new();
    digest.update(server_id);
    digest.update(shared_secret);
    digest.update(public_key_der);
    java_signed_hex(&digest.finalize())
}

fn java_signed_hex(bytes: &[u8]) -> String {
    BigInt::from_signed_bytes_be(bytes).to_str_radix(16)
}

// ---------------------------------------------------------------------------
// Mojang session verification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifySession {
    pub username: String,
    pub server_id_hash: String,
    pub client_ip: Option<IpAddr>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSession {
    pub uuid: uuid::Uuid,
    pub name: String,
    pub properties: Vec<GameProfileProperty>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum VerifySessionError {
    #[error("the session server did not verify this account")]
    Unverified,
    #[error("the session server is unavailable")]
    Unavailable,
}

pub type SessionVerifierFuture<'a> =
    Pin<Box<dyn Future<Output = Result<VerifiedSession, VerifySessionError>> + Send + 'a>>;

pub trait SessionVerifier: Send + Sync + fmt::Debug {
    fn verify(&self, request: VerifySession) -> SessionVerifierFuture<'_>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum SessionVerifierBuildError {
    #[error("invalid session verification URL")]
    InvalidBaseUrl,
    #[error("failed to construct HTTP client")]
    HttpClient,
}

#[derive(Debug, Clone)]
pub struct MojangSessionVerifier {
    client: reqwest::Client,
    base_url: reqwest::Url,
}

impl MojangSessionVerifier {
    pub fn new() -> Result<Self, SessionVerifierBuildError> {
        Self::with_base_url(
            MOJANG_HAS_JOINED_URL,
            DEFAULT_CONNECT_TIMEOUT,
            DEFAULT_REQUEST_TIMEOUT,
        )
    }

    pub fn with_base_url(
        base_url: &str,
        connect_timeout: Duration,
        request_timeout: Duration,
    ) -> Result<Self, SessionVerifierBuildError> {
        let base_url =
            reqwest::Url::parse(base_url).map_err(|_| SessionVerifierBuildError::InvalidBaseUrl)?;
        let client = reqwest::Client::builder()
            .connect_timeout(connect_timeout)
            .timeout(request_timeout)
            .build()
            .map_err(|_| SessionVerifierBuildError::HttpClient)?;
        Ok(Self { client, base_url })
    }
}

impl SessionVerifier for MojangSessionVerifier {
    fn verify(&self, request: VerifySession) -> SessionVerifierFuture<'_> {
        Box::pin(async move {
            let expected_username = request.username.clone();
            let mut query = vec![
                ("username", request.username),
                ("serverId", request.server_id_hash),
            ];
            if let Some(client_ip) = request.client_ip {
                query.push(("ip", client_ip.to_string()));
            }

            let response = self
                .client
                .get(self.base_url.clone())
                .query(&query)
                .send()
                .await
                .map_err(|_| VerifySessionError::Unavailable)?;
            let status = response.status();
            if status.is_server_error() {
                return Err(VerifySessionError::Unavailable);
            }
            if status.is_client_error() || status == reqwest::StatusCode::NO_CONTENT {
                return Err(VerifySessionError::Unverified);
            }
            if !status.is_success() {
                return Err(VerifySessionError::Unavailable);
            }

            let body = read_bounded_response_body(response).await?;
            let response: HasJoinedResponse =
                serde_json::from_slice(&body).map_err(|_| VerifySessionError::Unavailable)?;
            let uuid = response
                .id
                .ok_or(VerifySessionError::Unavailable)
                .and_then(|id| {
                    uuid::Uuid::parse_str(&id).map_err(|_| VerifySessionError::Unavailable)
                })?;
            let name = response.name.ok_or(VerifySessionError::Unavailable)?;
            if !name.eq_ignore_ascii_case(&expected_username) {
                return Err(VerifySessionError::Unverified);
            }
            let properties = validate_properties(response.properties.unwrap_or_default())?;
            Ok(VerifiedSession {
                uuid,
                name,
                properties,
            })
        })
    }
}

async fn read_bounded_response_body(
    mut response: reqwest::Response,
) -> Result<Vec<u8>, VerifySessionError> {
    if response
        .content_length()
        .is_some_and(|len| len > MAX_HAS_JOINED_BODY_BYTES as u64)
    {
        return Err(VerifySessionError::Unavailable);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| VerifySessionError::Unavailable)?
    {
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or(VerifySessionError::Unavailable)?;
        if next_len > MAX_HAS_JOINED_BODY_BYTES {
            return Err(VerifySessionError::Unavailable);
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn validate_properties(
    properties: Vec<HasJoinedProperty>,
) -> Result<Vec<GameProfileProperty>, VerifySessionError> {
    if properties.len() > MAX_PROFILE_PROPERTIES {
        return Err(VerifySessionError::Unavailable);
    }
    properties
        .into_iter()
        .map(|property| {
            if property.name.is_empty()
                || property.name.len() > MAX_PROPERTY_NAME_BYTES
                || property.value.len() > MAX_PROPERTY_VALUE_BYTES
                || property
                    .signature
                    .as_ref()
                    .is_some_and(|signature| signature.len() > MAX_PROPERTY_SIGNATURE_BYTES)
            {
                return Err(VerifySessionError::Unavailable);
            }
            Ok(property.into_game_profile_property())
        })
        .collect()
}

#[derive(Debug, serde::Deserialize)]
struct HasJoinedResponse {
    id: Option<String>,
    name: Option<String>,
    #[serde(default)]
    properties: Option<Vec<HasJoinedProperty>>,
}

#[derive(Debug, serde::Deserialize)]
struct HasJoinedProperty {
    name: String,
    value: String,
    #[serde(default)]
    signature: Option<String>,
}

impl HasJoinedProperty {
    fn into_game_profile_property(self) -> GameProfileProperty {
        GameProfileProperty {
            name: self.name,
            value: self.value,
            signature: self.signature,
        }
    }
}

#[cfg(test)]
#[path = "crypto_tests.rs"]
mod tests;
