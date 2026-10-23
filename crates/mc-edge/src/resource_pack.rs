//! Vanilla resource packs are connection presentation, never native gameplay state.
use crate::config::ResourcePackSection;
use crate::configuration::ClientPrefs;
use crate::connection::Connection;
use mc_protocol::packets::configuration::{ConfigurationDisconnect, ServerboundResourcePack};
use mc_protocol::packets::resource_pack::ResourcePackPush;
use mc_protocol::packets::{Packet, ResourcePackAction};

pub(crate) fn validate(pack: &ResourcePackSection) -> anyhow::Result<()> {
    if pack.url.is_empty() {
        anyhow::ensure!(
            !pack.required,
            "resource_pack.required needs resource_pack.url"
        );
        return Ok(());
    }
    let url = reqwest::Url::parse(&pack.url)?;
    anyhow::ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "resource pack URL must be HTTP(S)"
    );
    anyhow::ensure!(pack.url.len() <= 32767, "resource pack URL too long");
    anyhow::ensure!(
        pack.sha1.is_empty()
            || (pack.sha1.len() == 40 && pack.sha1.bytes().all(|b| b.is_ascii_hexdigit())),
        "resource pack sha1 must be empty or 40 hexadecimal digits"
    );
    let _ = prompt(pack)?;
    Ok(())
}

fn prompt(pack: &ResourcePackSection) -> anyhow::Result<Option<mc_nbt::Tag>> {
    if pack.prompt.is_empty() {
        return Ok(None);
    }
    anyhow::ensure!(pack.prompt.len() <= 32767, "resource pack prompt too long");
    let value: serde_json::Value = serde_json::from_str(&pack.prompt)?;
    anyhow::ensure!(
        value.is_string() || value.is_object() || value.is_array(),
        "resource pack prompt must be a text component"
    );
    component(value).map(Some)
}

fn component(value: serde_json::Value) -> anyhow::Result<mc_nbt::Tag> {
    use mc_nbt::{ListTag, Tag, tag_type};
    use serde_json::Value;
    Ok(match value {
        Value::String(s) => Tag::String(s),
        Value::Bool(b) => Tag::Byte(i8::from(b)),
        Value::Number(n) => {
            if let Some(n) = n.as_i64() {
                Tag::Long(n)
            } else {
                Tag::Double(n.as_f64().unwrap_or_default())
            }
        }
        Value::Object(fields) => Tag::Compound(
            fields
                .into_iter()
                .map(|(k, v)| Ok((k, component(v)?)))
                .collect::<anyhow::Result<_>>()?,
        ),
        Value::Array(values) => Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: values
                .into_iter()
                .map(|v| {
                    let tag = component(v)?;
                    Ok(match tag {
                        Tag::String(s) => Tag::Compound(vec![("text".into(), Tag::String(s))]),
                        Tag::Compound(_) => tag,
                        _ => anyhow::bail!(
                            "resource pack component arrays require strings or objects"
                        ),
                    })
                })
                .collect::<anyhow::Result<_>>()?,
        }),
        Value::Null => anyhow::bail!("resource pack prompt cannot contain null"),
    })
}

pub(crate) async fn negotiate(
    conn: &mut Connection,
    pack: &ResourcePackSection,
    prefs: &mut ClientPrefs,
) -> anyhow::Result<()> {
    if pack.url.is_empty() {
        return Ok(());
    }
    use md5::Digest;
    let id = pack.id.unwrap_or_else(|| {
        let mut bytes: [u8; 16] = md5::Md5::digest(pack.url.as_bytes()).into();
        bytes[6] = (bytes[6] & 0x0f) | 0x30;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        uuid::Uuid::from_bytes(bytes)
    });
    conn.send_packet(&ResourcePackPush {
        id,
        url: pack.url.clone(),
        sha1: pack.sha1.clone(),
        required: pack.required,
        prompt: prompt(pack)?,
    })
    .await?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(120), async {
        let mut packets = 0;
        loop {
            let frame = conn
                .read_frame_in(mc_protocol::State::Configuration)
                .await?;
            packets += 1;
            anyhow::ensure!(
                packets <= 32,
                "too many packets during resource pack negotiation"
            );
            if frame.id != ServerboundResourcePack::ID {
                crate::configuration::note_ignored(prefs, frame.id, frame.body)?;
                continue;
            }
            let mut body = frame.body;
            let status = ServerboundResourcePack::decode(&mut body)?.status;
            anyhow::ensure!(
                body.is_empty() && status.id == id,
                "invalid resource pack status"
            );
            if !status.action.is_terminal() {
                continue;
            }
            anyhow::ensure!(
                !pack.required || status.action == ResourcePackAction::SuccessfullyLoaded,
                "required resource pack was declined or failed to load"
            );
            return Ok::<_, anyhow::Error>(());
        }
    })
    .await;
    let error = match result {
        Ok(Ok(())) => return Ok(()),
        Ok(Err(error)) => error,
        Err(_) => anyhow::anyhow!("resource pack negotiation timed out"),
    };
    conn.send_packet(&ConfigurationDisconnect {
        reason_nbt: crate::connection::text_component_nbt(&error.to_string())?,
    })
    .await?;
    Err(error)
}

#[cfg(test)]
#[path = "resource_pack_tests.rs"]
mod tests;
