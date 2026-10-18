//! Shared Loader protocol transport for v2 configuration sessions.

use std::collections::BTreeSet;

use mc_loader::{LoaderArtifactRequest, LoaderClientAck, LoaderManifest, LoaderSession};
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundCustomPayload, ConfigurationDisconnect,
    ServerboundCustomPayload,
};
use mc_protocol::packets::{CustomPayload, Packet};

use crate::connection::{Connection, text_component_nbt};

pub(crate) async fn negotiate(
    conn: &mut Connection,
    manifest: &LoaderManifest,
    view_distance: &mut i8,
) -> anyhow::Result<LoaderSession> {
    let result = negotiate_inner(conn, manifest, view_distance).await;
    if let Err(error) = &result {
        let reason = format!(
            "This server requires the Solaris Loader client mod. Install it and reconnect. \
             (Loader negotiation failed: {error})"
        );
        if let Ok(reason_nbt) = text_component_nbt(&reason) {
            let _ = conn
                .send_packet(&ConfigurationDisconnect { reason_nbt })
                .await;
        }
    }
    result
}

async fn negotiate_inner(
    conn: &mut Connection,
    manifest: &LoaderManifest,
    view_distance: &mut i8,
) -> anyhow::Result<LoaderSession> {
    conn.send_packet(&ClientboundCustomPayload {
        payload: CustomPayload::Unknown {
            channel: mc_loader::loader_manifest_channel().clone(),
            payload: manifest.encode()?,
        },
    })
    .await?;
    let mut requests = BTreeSet::new();
    let mut ignored = 0usize;
    loop {
        let frame = conn
            .read_frame_in(mc_protocol::State::Configuration)
            .await?;
        anyhow::ensure!(
            frame.id != AcknowledgeFinishConfiguration::ID,
            "client finished configuration before acknowledging required Loader packages"
        );
        if frame.id == ServerboundCustomPayload::ID {
            let mut body = frame.body.clone();
            let payload = CustomPayload::decode_serverbound(&mut body)?;
            anyhow::ensure!(body.is_empty(), "trailing Loader custom payload bytes");
            if let CustomPayload::Unknown { channel, payload } = payload {
                if channel == *mc_loader::loader_ack_channel() {
                    let ack = LoaderClientAck::decode(&payload)?;
                    return Ok(manifest.bind_ack(&ack)?);
                }
                if channel == *mc_loader::loader_request_channel() {
                    let request = LoaderArtifactRequest::decode(&payload)?;
                    let (bundle, artifact) = manifest.requested_artifact(&request)?;
                    anyhow::ensure!(
                        requests.insert(request.cache_key.clone()),
                        "duplicate Loader artifact request"
                    );
                    anyhow::ensure!(
                        artifact.len() as u64 == bundle.size_bytes,
                        "Loader artifact size no longer matches manifest"
                    );
                    for (index, chunk) in artifact
                        .chunks(mc_loader::LOADER_ARTIFACT_CHUNK_BYTES)
                        .enumerate()
                    {
                        let offset = index * mc_loader::LOADER_ARTIFACT_CHUNK_BYTES;
                        let last = offset + chunk.len() == artifact.len();
                        conn.send_packet(&ClientboundCustomPayload {
                            payload: CustomPayload::Unknown {
                                channel: mc_loader::loader_artifact_channel().clone(),
                                payload: mc_loader::encode_artifact_chunk(
                                    &bundle.cache_key,
                                    offset as u64,
                                    last,
                                    chunk,
                                )?,
                            },
                        })
                        .await?;
                    }
                    continue;
                }
            }
        }
        crate::configuration::note_ignored(view_distance, frame.id, frame.body)?;
        ignored += 1;
        anyhow::ensure!(ignored <= 32, "too many unrelated Loader handshake packets");
    }
}
