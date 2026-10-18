//! Headless Loader transport: download and verify actual artifacts before ACK.
//! This enables protocol gameplay, not rendered UI or asset-execution evidence.
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use bytes::Buf;
use mc_loader::{
    LoaderArtifactRequest, LoaderBundle, LoaderClientAck, LoaderManifest, LoaderPlatform,
};
use mc_protocol::packets::configuration::{
    AcknowledgeFinishConfiguration, ClientboundCustomPayload, ClientboundKnownPacks,
    FinishConfiguration, RegistryData, ServerboundCustomPayload, ServerboundKnownPacks,
    UpdateEnabledFeatures, UpdateTags,
};
use mc_protocol::packets::{CustomPayload, Packet};
use mc_vanilla::wire_client::Client;
use sha2::{Digest, Sha256};

const MAX_ARTIFACT_BYTES: u64 = 64 * 1024 * 1024;

struct Download {
    file: std::fs::File,
    digest: Sha256,
    received: u64,
}

impl Download {
    fn chunk(&mut self, bundle: &LoaderBundle, payload: &[u8]) -> Result<bool> {
        let mut data = payload;
        ensure!(data.remaining() >= 4, "truncated Loader artifact header");
        ensure!(
            data.get_u16() == mc_loader::LOADER_PROTOCOL_VERSION,
            "Loader artifact protocol mismatch"
        );
        let key_len = usize::from(data.get_u16());
        ensure!(
            data.remaining() >= key_len + 9,
            "truncated Loader artifact identity"
        );
        ensure!(
            &data[..key_len] == bundle.cache_key.as_bytes(),
            "unexpected Loader artifact identity"
        );
        data.advance(key_len);
        ensure!(
            data.get_u64() == self.received,
            "noncontiguous Loader artifact chunk"
        );
        let last = data.get_u8();
        ensure!(
            last <= 1 && !data.is_empty() && data.len() <= mc_loader::LOADER_ARTIFACT_CHUNK_BYTES,
            "invalid Loader artifact chunk"
        );
        self.received = self
            .received
            .checked_add(data.len() as u64)
            .context("Loader size overflow")?;
        ensure!(
            self.received <= bundle.size_bytes && bundle.size_bytes <= MAX_ARTIFACT_BYTES,
            "Loader artifact exceeds declared/bounded size"
        );
        self.file.write_all(data)?;
        self.digest.update(data);
        if last == 1 {
            ensure!(
                self.received == bundle.size_bytes,
                "Loader artifact incomplete"
            );
            let actual = format!("{:x}", self.digest.clone().finalize());
            ensure!(actual == bundle.sha256, "Loader artifact SHA-256 mismatch");
            self.file.sync_all()?;
            return Ok(true);
        }
        ensure!(
            self.received < bundle.size_bytes,
            "Loader artifact missing final marker"
        );
        Ok(false)
    }
}

async fn payload(client: &mut Client, channel: &mc_data::Identifier, data: Vec<u8>) -> Result<()> {
    client
        .write_packet(&ServerboundCustomPayload {
            payload: CustomPayload::Unknown {
                channel: channel.clone(),
                payload: data,
            },
        })
        .await
}

async fn negotiate(client: &mut Client, manifest: LoaderManifest, cache: &Path) -> Result<()> {
    ensure!(
        manifest.protocol == mc_loader::LOADER_PROTOCOL_VERSION,
        "unsupported Loader protocol"
    );
    std::fs::create_dir_all(cache)?;
    let mut accepted = BTreeSet::new();
    let mut cached = Vec::with_capacity(manifest.bundles.len());
    let mut seen = BTreeSet::new();
    for bundle in &manifest.bundles {
        ensure!(
            bundle.loaders.contains(&LoaderPlatform::Native),
            "bundle does not support native protocol bots"
        );
        ensure!(
            seen.insert(&bundle.cache_key),
            "duplicate Loader cache identity"
        );
        ensure!(
            bundle.size_bytes > 0 && bundle.size_bytes <= MAX_ARTIFACT_BYTES,
            "Loader artifact size outside bot bound"
        );
        ensure!(
            bundle.sha256.len() == 64 && bundle.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid Loader artifact SHA-256"
        );
        // Cache filenames come only from a validated digest, never a server path.
        let partial = cache.join(format!("{}.partial", bundle.sha256));
        let complete = cache.join(format!("{}.zip", bundle.sha256));
        let mut download = Download {
            file: std::fs::File::create(&partial)?,
            digest: Sha256::new(),
            received: 0,
        };
        payload(
            client,
            mc_loader::loader_request_channel(),
            serde_json::to_vec(&LoaderArtifactRequest {
                protocol: manifest.protocol,
                cache_key: bundle.cache_key.clone(),
            })?,
        )
        .await?;
        loop {
            let frame = client.read_frame().await?;
            ensure!(
                frame.id == ClientboundCustomPayload::ID,
                "expected Loader artifact custom payload"
            );
            let mut body = frame.body.as_ref();
            let packet = CustomPayload::decode(&mut body)?;
            ensure!(body.is_empty(), "trailing Loader payload bytes");
            let CustomPayload::Unknown {
                channel,
                payload: data,
            } = packet
            else {
                bail!("expected Loader artifact channel");
            };
            ensure!(
                channel == *mc_loader::loader_artifact_channel(),
                "unexpected Loader download channel"
            );
            if download.chunk(bundle, &data)? {
                break;
            }
        }
        drop(download);
        std::fs::rename(partial, complete)?;
        accepted.extend(bundle.permissions.iter().copied());
        cached.push(bundle.cache_key.clone());
    }
    let ack = LoaderClientAck {
        protocol: manifest.protocol,
        platform: LoaderPlatform::Native,
        loader_version: "solaris-protocol-bots-0.0.8".into(),
        accepted_permissions: accepted.into_iter().collect(),
        cached_bundles: cached,
        carrier_block_state_ids: BTreeMap::new(),
    };
    manifest.validate_ack(&ack)?;
    std::fs::write(cache.join("receipt.json"), serde_json::to_vec_pretty(&ack)?)?;
    payload(
        client,
        mc_loader::loader_ack_channel(),
        serde_json::to_vec(&ack)?,
    )
    .await
}

pub(super) async fn configure(client: &mut Client, cache: &Path) -> Result<()> {
    let mut manifest_received = false;
    loop {
        let frame = client.read_frame().await?;
        if frame.id == ClientboundKnownPacks::ID {
            let packs = ClientboundKnownPacks::decode(&mut frame.body.as_ref())?;
            client
                .write_packet(&ServerboundKnownPacks { packs: packs.packs })
                .await?;
        } else if frame.id == UpdateEnabledFeatures::ID {
            UpdateEnabledFeatures::decode(&mut frame.body.as_ref())?;
        } else if frame.id == RegistryData::ID || frame.id == UpdateTags::ID {
            continue;
        } else if frame.id == ClientboundCustomPayload::ID {
            let mut body = frame.body.as_ref();
            let packet = CustomPayload::decode(&mut body)?;
            ensure!(body.is_empty(), "trailing configuration payload bytes");
            if let CustomPayload::Unknown { channel, payload } = packet
                && channel == *mc_loader::loader_manifest_channel()
            {
                ensure!(!manifest_received, "duplicate Loader manifest");
                ensure!(
                    payload.len() <= mc_loader::MAX_LOADER_MANIFEST_BYTES,
                    "Loader manifest too large"
                );
                manifest_received = true;
                negotiate(client, serde_json::from_slice(&payload)?, cache).await?;
            }
        } else if frame.id == FinishConfiguration::ID {
            client.write_packet(&AcknowledgeFinishConfiguration).await?;
            return Ok(());
        } else {
            bail!("unexpected configuration frame id=0x{:02x}", frame.id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn vanilla_features_before_known_packs_preserve_configuration_acks() -> Result<()> {
        use mc_protocol::frame::{Compression, encode_frame, try_decode_frame};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let mut client = Client::connect(listener.local_addr()?).await?;
        let (mut peer, _) = listener.accept().await?;
        let mut body = Vec::new();
        UpdateEnabledFeatures {
            features: vec![mc_data::Identifier::parse("minecraft:vanilla")?],
        }
        .encode(&mut body)?;
        peer.write_all(&encode_frame(
            UpdateEnabledFeatures::ID,
            &body,
            Compression::Disabled,
        )?)
        .await?;
        body.clear();
        ClientboundKnownPacks { packs: Vec::new() }.encode(&mut body)?;
        peer.write_all(&encode_frame(
            ClientboundKnownPacks::ID,
            &body,
            Compression::Disabled,
        )?)
        .await?;
        peer.write_all(&encode_frame(
            FinishConfiguration::ID,
            &[],
            Compression::Disabled,
        )?)
        .await?;
        let cache = tempfile::tempdir()?;
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            configure(&mut client, cache.path()),
        )
        .await??;
        let mut received = bytes::BytesMut::new();
        for id in [
            ServerboundKnownPacks::ID,
            AcknowledgeFinishConfiguration::ID,
        ] {
            let frame = loop {
                if let Some(frame) = try_decode_frame(&mut received, Compression::Disabled)? {
                    break frame;
                }
                ensure!(
                    peer.read_buf(&mut received).await? != 0,
                    "configuration client closed"
                );
            };
            assert_eq!(frame.id, id);
        }
        assert!(!cache.path().join("receipt.json").exists());
        Ok(())
    }

    #[test]
    fn artifact_chunks_require_exact_identity_order_size_and_hash() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"actual downloaded artifact";
        let bundle: LoaderBundle = serde_json::from_value(serde_json::json!({
            "owner":"example", "id":"ui", "version":"1", "artifact":"client/ui.zip",
            "sha256":format!("{:x}", Sha256::digest(bytes)), "size_bytes":bytes.len(),
            "loaders":["native"], "content":["views"], "permissions":["present_views"], "cache_key":"example:ui/1/verified"
        })).unwrap();
        let make = || Download {
            file: std::fs::File::create(dir.path().join("download")).unwrap(),
            digest: Sha256::new(),
            received: 0,
        };
        let mut download = make();
        assert!(
            !download
                .chunk(
                    &bundle,
                    &mc_loader::encode_artifact_chunk(&bundle.cache_key, 0, false, &bytes[..5])
                        .unwrap()
                )
                .unwrap()
        );
        assert!(
            download
                .chunk(
                    &bundle,
                    &mc_loader::encode_artifact_chunk(&bundle.cache_key, 5, true, &bytes[5..])
                        .unwrap()
                )
                .unwrap()
        );
        for (key, offset, data) in [
            ("wrong", 0, bytes.as_slice()),
            (&*bundle.cache_key, 1, bytes.as_slice()),
            (&*bundle.cache_key, 0, b"corrupted bytes".as_slice()),
        ] {
            assert!(
                make()
                    .chunk(
                        &bundle,
                        &mc_loader::encode_artifact_chunk(key, offset, true, data).unwrap()
                    )
                    .is_err()
            );
        }
        let mut wrong_hash = bundle.clone();
        wrong_hash.sha256 = "0".repeat(64);
        assert!(
            make()
                .chunk(
                    &wrong_hash,
                    &mc_loader::encode_artifact_chunk(&bundle.cache_key, 0, true, bytes).unwrap()
                )
                .is_err()
        );
    }
}
