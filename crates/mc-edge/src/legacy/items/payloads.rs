//! Payloads emitted by Solaris's copied/station component codecs. Layout
//! authority: mc-protocol::packets::play::{copied_components,station_components}.
use super::*;

pub(super) fn registry(
    t: &Tables,
    to: To,
    name: &str,
    r: &mut &[u8],
    w: &mut Vec<u8>,
    holder: bool,
) -> Result<()> {
    let raw = r.read_varint()?;
    let offset = i32::from(holder);
    ensure!(
        raw >= offset,
        "inline {name} holder is not supported by Solaris"
    );
    let ids = t
        .registry_ids
        .get(name)
        .with_context(|| format!("missing {name} translation"))?;
    let id = map(ids, to, raw - offset).with_context(|| format!("unmapped {name} id {raw}"))?;
    w.write_varint(id + offset);
    Ok(())
}

fn string(r: &mut &[u8], w: &mut Vec<u8>) -> Result<()> {
    w.write_string(&r.read_string(32767)?, 32767)?;
    Ok(())
}

fn optional(
    r: &mut &[u8],
    w: &mut Vec<u8>,
    copy: impl FnOnce(&mut &[u8], &mut Vec<u8>) -> Result<()>,
) -> Result<()> {
    let present = r.read_bool()?;
    w.write_bool(present);
    if present {
        copy(r, w)?;
    }
    Ok(())
}

fn filtered(r: &mut &[u8], w: &mut Vec<u8>, text: bool) -> Result<()> {
    if text {
        copy_nbt(r, w)?;
    } else {
        string(r, w)?;
    }
    optional(
        r,
        w,
        |r, w| if text { copy_nbt(r, w) } else { string(r, w) },
    )
}

pub(super) fn extra(
    t: &Tables,
    to: To,
    shape: Payload,
    r: &mut &[u8],
    w: &mut Vec<u8>,
    depth: usize,
) -> Result<()> {
    match shape {
        Payload::Container => {
            let n = list_len(r, w)?;
            for _ in 0..n {
                optional(r, w, |r, w| template_at(t, to, r, w, depth + 1))?;
            }
        }
        Payload::PotDecorations => match to {
            To::Legacy => {
                w.write_varint(4);
                for _ in 0..4 {
                    if r.read_bool()? {
                        let mut out = Vec::new();
                        template_at(t, to, r, &mut out, depth + 1)?;
                        w.write_varint(out.as_slice().read_varint()?);
                    } else {
                        w.write_varint(t.legacy_brick);
                    }
                }
            }
            To::Canonical => {
                let n = r.read_varint()?;
                ensure!((0..=4).contains(&n), "pot decoration count");
                for side in 0..4 {
                    let id = if side < n {
                        r.read_varint()?
                    } else {
                        t.legacy_brick
                    };
                    w.write_bool(true);
                    w.write_varint(t.items.back(id).context("unmapped pot item")?);
                    w.write_varint(1);
                    w.write_varint(0);
                    w.write_varint(0);
                }
            }
        },
        Payload::DeathProtection => {
            // Native admission currently permits only an empty ConsumeEffect
            // list. Never treat an unknown nonempty list as an empty one.
            let n = r.read_varint()?;
            ensure!(
                n == 0,
                "nonempty death protection is unsupported by Solaris"
            );
            w.write_varint(n);
        }
        Payload::StewEffects => {
            let n = list_len(r, w)?;
            for _ in 0..n {
                registry(t, to, "mob_effect", r, w, false)?;
                w.write_varint(r.read_varint()?);
            }
        }
        Payload::WritableBook | Payload::WrittenBook => {
            let written = shape == Payload::WrittenBook;
            if written {
                filtered(r, w, false)?;
                string(r, w)?;
                w.write_varint(r.read_varint()?);
            }
            let n = list_len(r, w)?;
            for _ in 0..n {
                filtered(r, w, written)?;
            }
            if written {
                w.write_bool(r.read_bool()?);
            }
        }
        Payload::Trim => {
            registry(t, to, "trim_material", r, w, true)?;
            registry(t, to, "trim_pattern", r, w, true)?;
        }
        Payload::BannerPatterns => {
            let n = list_len(r, w)?;
            for _ in 0..n {
                registry(t, to, "banner_pattern", r, w, true)?;
                w.write_varint(r.read_varint()?);
            }
        }
        Payload::Fireworks => {
            w.write_varint(r.read_varint()?);
            let n = list_len(r, w)?;
            for _ in 0..n {
                w.write_varint(r.read_varint()?);
                for _ in 0..2 {
                    let n = list_len(r, w)?;
                    for _ in 0..n {
                        w.write_i32(r.read_i32()?);
                    }
                }
                w.write_bool(r.read_bool()?);
                w.write_bool(r.read_bool()?);
            }
        }
        Payload::Profile => {
            let full = r.read_bool()?;
            w.write_bool(full);
            if full {
                w.write_uuid(r.read_uuid()?);
                string(r, w)?;
            } else {
                optional(r, w, string)?;
                optional(r, w, |r, w| {
                    w.write_uuid(r.read_uuid()?);
                    Ok(())
                })?;
            }
            let n = list_len(r, w)?;
            for _ in 0..n {
                string(r, w)?;
                string(r, w)?;
                optional(r, w, string)?;
            }
            for _ in 0..3 {
                optional(r, w, string)?;
            }
            optional(r, w, |r, w| {
                w.write_bool(r.read_bool()?);
                Ok(())
            })?;
        }
        Payload::Bees => {
            let n = list_len(r, w)?;
            for _ in 0..n {
                let id = map(&t.entities, to, r.read_varint()?).context("unmapped bee entity")?;
                w.write_varint(id);
                copy_nbt(r, w)?;
                w.write_varint(r.read_varint()?);
                w.write_varint(r.read_varint()?);
            }
        }
        _ => bail!("not an extended component payload: {shape:?}"),
    }
    Ok(())
}
