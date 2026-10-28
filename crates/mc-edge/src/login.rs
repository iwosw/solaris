//! Login state handler for offline and Mojang-authenticated online mode.
//!
//! Access-list admission happens after profile authentication and before
//! LoginSuccess, so rejected clients receive a LoginDisconnect.
//!
//! Online mode performs the RSA challenge, enables the continuous
//! AES-CFB8 transport, verifies the account with the session service, and
//! only then enables compression and waits for the acknowledgement.

use std::fmt;
use std::net::IpAddr;

use mc_protocol::frame::Compression;
use mc_protocol::packets::login::{
    EncryptionRequest, EncryptionResponse, GameProfileProperty, LoginAcknowledged, LoginDisconnect,
    LoginPluginRequest, LoginPluginResponse, LoginStart, LoginSuccess, SetCompression,
};

use crate::connection::Connection;
use crate::crypto::{
    MojangSessionVerifier, RsaIdentity, SessionVerifier, VerifySession, VerifySessionError,
    minecraft_server_hash, offline_uuid,
};

/// Default `SetCompression` threshold for the login flow.
pub const DEFAULT_COMPRESSION_THRESHOLD: i32 = 256;

/// 26.3 `ServerConnectionListener.getSessionId()`: one random UUID per server
/// process, carried by every `ClientboundLoginFinishedPacket`.
static SERVER_SESSION_ID: std::sync::LazyLock<uuid::Uuid> = std::sync::LazyLock::new(|| {
    use rsa::rand_core::{OsRng, RngCore};
    let mut bytes = [0_u8; 16];
    OsRng.fill_bytes(&mut bytes);
    uuid::Builder::from_random_bytes(bytes).into_uuid()
});

/// Options controlling the login flow.
#[derive(Debug, Clone)]
pub struct LoginOpts {
    pub online_mode: bool,
    pub compression_threshold: i32,
    /// Verified Velocity modern-forwarding secret; `Some` requires the proxy
    /// plugin-message handshake instead of Mojang authentication.
    pub velocity: Option<crate::proxy::VelocityForwarding>,
}

/// Outcome of a successful login, returned for the transition into
/// Configuration.
#[derive(Debug, Clone)]
pub struct LoginOutcome {
    pub uuid: uuid::Uuid,
    pub name: String,
    pub properties: Vec<GameProfileProperty>,
    /// Client address the proxy forwarded, when one authenticated the login.
    pub remote_address: Option<IpAddr>,
}

/// Name validation failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginNameError {
    Length { bytes: usize },
    InvalidCharacter { index: usize, byte: u8 },
}

pub fn validate_login_name(name: &str) -> Result<(), LoginNameError> {
    if !(3..=16).contains(&name.len()) {
        return Err(LoginNameError::Length { bytes: name.len() });
    }
    for (index, byte) in name.bytes().enumerate() {
        if !byte.is_ascii_alphanumeric() && byte != b'_' {
            return Err(LoginNameError::InvalidCharacter { index, byte });
        }
    }
    Ok(())
}

/// Online-mode authentication context (RSA identity + session verifier).
#[derive(Debug)]
pub struct OnlineAuthentication {
    identity: RsaIdentity,
    verifier: Box<dyn SessionVerifier>,
    prevent_proxy_connections: bool,
}

impl OnlineAuthentication {
    pub fn new(
        verifier: Box<dyn SessionVerifier>,
        prevent_proxy_connections: bool,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            identity: RsaIdentity::generate()?,
            verifier,
            prevent_proxy_connections,
        })
    }

    pub fn with_mojang_verifier(prevent_proxy_connections: bool) -> anyhow::Result<Self> {
        let verifier = MojangSessionVerifier::new()?;
        Self::new(Box::new(verifier), prevent_proxy_connections)
    }
}

/// Handles the login state. Returns `None` when the login was rejected
/// (disconnect packet already sent); the caller closes TCP.
pub async fn handle_login(
    conn: &mut Connection,
    opts: &LoginOpts,
) -> anyhow::Result<Option<LoginOutcome>> {
    handle_login_with_auth(conn, opts, None, None).await
}

/// [`handle_login`] with an explicit online-mode authentication context and
/// peer address. `None` authentication in online mode is a server logic
/// error and returns `Err`. `peer_ip` feeds the Mojang
/// `prevent_proxy_connections` check.
pub async fn handle_login_with_auth(
    conn: &mut Connection,
    opts: &LoginOpts,
    online_authentication: Option<&OnlineAuthentication>,
    peer_ip: Option<IpAddr>,
) -> anyhow::Result<Option<LoginOutcome>> {
    handle_login_admitted(conn, opts, online_authentication, peer_ip, |_, _| None).await
}

pub(crate) async fn handle_login_admitted(
    conn: &mut Connection,
    opts: &LoginOpts,
    online_authentication: Option<&OnlineAuthentication>,
    peer_ip: Option<IpAddr>,
    admission: impl FnOnce(uuid::Uuid, Option<IpAddr>) -> Option<serde_json::Value>,
) -> anyhow::Result<Option<LoginOutcome>> {
    let login_start = conn
        .read_packet::<LoginStart>(mc_protocol::State::Login)
        .await?;
    let requested_name = login_start.name;
    if validate_login_name(&requested_name).is_err() {
        write_login_disconnect(conn, "Invalid username").await?;
        return Ok(None);
    }

    let (name, uuid, properties, remote_address) = if let Some(velocity) = opts.velocity.as_ref() {
        if opts.online_mode {
            anyhow::bail!("proxy forwarding and Mojang online mode are mutually exclusive");
        }
        // Velocity answers `velocity:player_info` with the signed profile.
        // A vanilla client answers `successful = false`, so a direct
        // connection cannot impersonate a proxied one.
        const MESSAGE_ID: i32 = 0;
        conn.send_packet(&LoginPluginRequest {
            message_id: MESSAGE_ID,
            channel: mc_protocol::codec::Identifier::parse(crate::proxy::CHANNEL)
                .expect("static forwarding channel identifier"),
            data: vec![velocity.request_version()],
        })
        .await?;
        let response = conn
            .read_packet::<LoginPluginResponse>(mc_protocol::State::Login)
            .await?;
        let forwarded = if response.message_id == MESSAGE_ID && response.successful {
            velocity.verify_and_parse(&response.data).ok()
        } else {
            None
        };
        let Some(forwarded) = forwarded else {
            write_login_disconnect(conn, "This server requires you to connect with Velocity.")
                .await?;
            return Ok(None);
        };
        if validate_login_name(&forwarded.name).is_err()
            || !forwarded.name.eq_ignore_ascii_case(&requested_name)
        {
            write_login_disconnect(conn, "This server requires you to connect with Velocity.")
                .await?;
            return Ok(None);
        }
        let remote_address = forwarded.ip();
        (
            forwarded.name,
            forwarded.uuid,
            forwarded.properties,
            remote_address,
        )
    } else if opts.online_mode {
        let Some(authentication) = online_authentication else {
            anyhow::bail!("online mode requires an authentication context");
        };
        let challenge = authentication.identity.new_challenge();
        conn.send_packet(&EncryptionRequest {
            server_id: String::new(),
            public_key: authentication.identity.public_key_der().to_vec(),
            verify_token: challenge.to_vec(),
            should_authenticate: true,
        })
        .await?;
        let response = conn
            .read_packet::<EncryptionResponse>(mc_protocol::State::Login)
            .await?;
        if response.encrypted_shared_secret.len() > 128
            || response.encrypted_verify_token.len() > 128
        {
            write_login_disconnect(conn, "Invalid encryption response").await?;
            return Ok(None);
        }
        let Ok(shared_secret) = authentication.identity.decrypt_response(
            &response.encrypted_shared_secret,
            &response.encrypted_verify_token,
            challenge,
        ) else {
            write_login_disconnect(conn, "Invalid encryption response").await?;
            return Ok(None);
        };
        conn.enable_encryption(&shared_secret);

        let verified = authentication
            .verifier
            .verify(VerifySession {
                username: requested_name.clone(),
                server_id_hash: minecraft_server_hash(
                    b"",
                    &shared_secret,
                    authentication.identity.public_key_der(),
                ),
                client_ip: authentication
                    .prevent_proxy_connections
                    .then_some(peer_ip)
                    .flatten(),
            })
            .await;
        let crate::crypto::VerifiedSession {
            uuid,
            name,
            properties,
        } = match verified {
            Ok(profile) => profile,
            Err(VerifySessionError::Unverified) => {
                write_login_disconnect(conn, "Failed to verify username!").await?;
                return Ok(None);
            }
            Err(VerifySessionError::Unavailable) => {
                write_login_disconnect(
                    conn,
                    "Authentication servers are down. Please try again later, sorry!",
                )
                .await?;
                return Ok(None);
            }
        };
        if validate_login_name(&name).is_err() || !name.eq_ignore_ascii_case(&requested_name) {
            write_login_disconnect(conn, "Failed to verify username!").await?;
            return Ok(None);
        }
        (name, uuid, properties, None)
    } else {
        // Offline mode ignores the client UUID and derives the vanilla UUID.
        let uuid = offline_uuid(&requested_name);
        (requested_name, uuid, Vec::new(), None)
    };
    if let Some(reason) = admission(uuid, remote_address.or(peer_ip)) {
        conn.send_packet(&LoginDisconnect {
            reason_json: reason.to_string(),
        })
        .await?;
        return Ok(None);
    }

    let compression_threshold = opts.compression_threshold.max(0);
    conn.send_packet(&SetCompression {
        threshold: compression_threshold,
    })
    .await?;
    conn.set_compression(Compression::Threshold(compression_threshold as usize));

    conn.send_packet(&LoginSuccess {
        uuid,
        name: name.clone(),
        properties: properties.clone(),
        session_id: *SERVER_SESSION_ID,
    })
    .await?;

    let _ = conn
        .read_packet::<LoginAcknowledged>(mc_protocol::State::Login)
        .await?;

    Ok(Some(LoginOutcome {
        uuid,
        name,
        properties,
        remote_address,
    }))
}

async fn write_login_disconnect(conn: &mut Connection, reason: &str) -> anyhow::Result<()> {
    conn.send_packet(&LoginDisconnect {
        reason_json: serde_json::json!({ "text": reason }).to_string(),
    })
    .await
}

impl fmt::Display for LoginNameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Length { bytes } => write!(f, "invalid name length: {bytes} bytes"),
            Self::InvalidCharacter { index, byte } => {
                write!(f, "invalid character {byte:#04x} at index {index}")
            }
        }
    }
}

impl std::error::Error for LoginNameError {}

#[cfg(test)]
#[path = "login_tests.rs"]
mod tests;
