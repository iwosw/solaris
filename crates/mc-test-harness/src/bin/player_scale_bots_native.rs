//! Read-only evidence from the server's committed native checkpoint.
//! No runtime owner, writer, procedural generator, or invented base is opened.
use anyhow::{Context, Result, ensure};
use mc_nbt::Tag;
use mc_world::native::NativeReadOnlyStore;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

fn exported_chunk(directory: &Path, x: i32, z: i32) -> Result<Value> {
    let path =
        directory
            .join("region")
            .join(format!("r.{}.{}.mca", x.div_euclid(32), z.div_euclid(32)));
    let mut file = std::fs::File::open(&path)?;
    let slot = x.rem_euclid(32) + z.rem_euclid(32) * 32;
    file.seek(SeekFrom::Start(u64::try_from(slot)? * 4))?;
    let mut location = [0_u8; 4];
    file.read_exact(&mut location)?;
    let sectors = u64::from(location[3]);
    let offset = u64::from(u32::from_be_bytes([
        0,
        location[0],
        location[1],
        location[2],
    ]));
    ensure!(
        offset >= 2 && sectors > 0,
        "exported native chunk absent: {x},{z}"
    );
    ensure!(
        (offset + sectors) * 4096 <= file.metadata()?.len(),
        "exported region extent invalid"
    );
    file.seek(SeekFrom::Start(offset * 4096))?;
    let mut header = [0_u8; 5];
    file.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header[..4].try_into()?);
    ensure!(
        length > 1 && u64::from(length) + 4 <= sectors * 4096,
        "exported chunk length invalid"
    );
    let mut encoded = vec![0; usize::try_from(length - 1)?];
    file.read_exact(&mut encoded)?;
    let mut bytes = Vec::new();
    match header[4] {
        1 => {
            flate2::read::GzDecoder::new(encoded.as_slice())
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
        }
        2 => {
            flate2::read::ZlibDecoder::new(encoded.as_slice())
                .take(16 * 1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
        }
        3 => bytes = encoded,
        kind => anyhow::bail!("unsupported native-export chunk compression {kind}"),
    }
    ensure!(
        bytes.len() <= 16 * 1024 * 1024,
        "native-export chunk exceeds NBT budget"
    );
    let (_, tag) = mc_nbt::read_named_document(&mut bytes.as_slice())?;
    let root = nbt_json(&tag)?;
    ensure!(
        root["xPos"].as_i64() == Some(i64::from(x)) && root["zPos"].as_i64() == Some(i64::from(z)),
        "native-export chunk coordinate mismatch"
    );
    Ok(root)
}

fn cached_chunk<'a>(
    cache: &'a mut BTreeMap<(i32, i32), Value>,
    directory: &Path,
    key: (i32, i32),
) -> Result<&'a Value> {
    Ok(match cache.entry(key) {
        std::collections::btree_map::Entry::Occupied(entry) => entry.into_mut(),
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(exported_chunk(directory, key.0, key.1)?)
        }
    })
}

fn exported_blocks(spec: &Value, cache: &mut BTreeMap<(i32, i32), Value>) -> Result<Value> {
    let Some(probes) = spec["blocks"]
        .as_array()
        .filter(|probes| !probes.is_empty())
    else {
        return Ok(json!({}));
    };
    let directory = Path::new(
        spec["native_anvil_snapshot"]
            .as_str()
            .context("physical native probes require the server's read-only export")?,
    );
    let data = Path::new(
        spec["native_data_dir"]
            .as_str()
            .context("native registry data directory missing")?,
    );
    let mut registry: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for block in mc_data::blocks::load_blocks_report(data.join("reports/blocks.json"))? {
        registry.insert(block.id.to_string(), block.states);
    }
    let empty_properties = Map::new();
    let mut result = Map::new();
    for probe in probes {
        let position = probe["position"]
            .as_array()
            .context("native physical position missing")?;
        ensure!(
            position.len() == 3,
            "native physical position length invalid"
        );
        let coordinate = |i: usize| -> Result<i32> {
            Ok(i32::try_from(
                position[i]
                    .as_i64()
                    .context("noninteger native physical position")?,
            )?)
        };
        let (x, y, z) = (coordinate(0)?, coordinate(1)?, coordinate(2)?);
        let chunk = cached_chunk(cache, directory, (x.div_euclid(16), z.div_euclid(16)))?;
        let section = chunk["sections"]
            .as_array()
            .context("native-export sections missing")?
            .iter()
            .find(|section| section["Y"].as_i64() == Some(i64::from(y.div_euclid(16))))
            .context("native-export requested section missing")?;
        let states = &section["block_states"];
        let palette = states["palette"]
            .as_array()
            .context("native-export block palette missing")?;
        ensure!(
            !palette.is_empty() && palette.len() <= 4096,
            "native block palette size invalid"
        );
        let index =
            usize::try_from(x.rem_euclid(16) + z.rem_euclid(16) * 16 + y.rem_euclid(16) * 256)?;
        let palette_index = if palette.len() == 1 {
            0
        } else {
            let bits = palette.len().next_power_of_two().trailing_zeros().max(4);
            let per_word = 64 / bits as usize;
            let words = states["data"]
                .as_array()
                .context("native palette data missing")?;
            let word = words
                .get(index / per_word)
                .and_then(Value::as_i64)
                .context("native block word missing")? as u64;
            usize::try_from((word >> ((index % per_word) * bits as usize)) & ((1_u64 << bits) - 1))?
        };
        let state = palette
            .get(palette_index)
            .context("native palette index invalid")?;
        let name = state["Name"]
            .as_str()
            .context("native block identity missing")?;
        let properties = state.get("Properties").and_then(Value::as_object);
        ensure!(
            state.get("Properties").is_none() || properties.is_some(),
            "native block properties are not a compound"
        );
        let properties = properties.unwrap_or(&empty_properties);
        let protocol_id = registry
            .get(name)
            .and_then(|states| {
                states.iter().find(|candidate| {
                    candidate.properties.len() == properties.len()
                        && candidate.properties.iter().all(|(key, value)| {
                            properties.get(key).and_then(Value::as_str) == Some(value.as_str())
                        })
                })
            })
            .map(|candidate| candidate.id)
            .context("native block descriptor absent from supplied registry")?;
        let label = probe["name"]
            .as_str()
            .context("native physical probe name missing")?;
        ensure!(
            !result.contains_key(label),
            "duplicate native physical probe name"
        );
        result.insert(
            label.to_owned(),
            json!({"name":label,"position":position,
            "final":{"state":protocol_id,"name":name,"properties":properties},
            "source":"server read-only native export"}),
        );
    }
    Ok(Value::Object(result))
}

fn exported_containers(spec: &Value, chunks: &mut BTreeMap<(i32, i32), Value>) -> Result<Value> {
    let Some(probes) = spec["containers"]
        .as_array()
        .filter(|probes| !probes.is_empty())
    else {
        return Ok(json!({}));
    };
    let directory = Path::new(
        spec["native_anvil_snapshot"]
            .as_str()
            .context("container native probes require the server's read-only export")?,
    );
    let mut result = Map::new();
    for probe in probes {
        let position = probe["position"]
            .as_array()
            .context("native container position missing")?;
        ensure!(
            position.len() == 3,
            "native container position length invalid"
        );
        let x = i32::try_from(position[0].as_i64().context("native container x missing")?)?;
        let z = i32::try_from(position[2].as_i64().context("native container z missing")?)?;
        let key = (x.div_euclid(16), z.div_euclid(16));
        let chunk = cached_chunk(chunks, directory, key)?;
        let entries = chunk["block_entities"]
            .as_array()
            .context("native-export block entity list missing")?;
        let block = entries
            .iter()
            .find(|entry| {
                entry["x"] == position[0] && entry["y"] == position[1] && entry["z"] == position[2]
            })
            .context("native-export requested container missing")?;
        let count = usize::try_from(
            probe["slots"]
                .as_u64()
                .context("container slot bound missing")?,
        )?;
        ensure!((1..=54).contains(&count), "container slot bound invalid");
        let mut slots = (0..count)
            .map(|slot| json!({"slot":slot,"name":"minecraft:air","count":0}))
            .collect::<Vec<_>>();
        let mut seen = std::collections::BTreeSet::new();
        for item in block["Items"]
            .as_array()
            .context("native container Items missing")?
        {
            let slot = usize::try_from(item["Slot"].as_u64().context("native item slot missing")?)?;
            ensure!(
                slot < count && seen.insert(slot),
                "native container duplicate/out-of-range slot"
            );
            let amount = item
                .get("count")
                .or_else(|| item.get("Count"))
                .and_then(Value::as_u64)
                .context("native item count missing")?;
            let name = item["id"]
                .as_str()
                .context("native item identity missing")?;
            ensure!(
                amount > 0 && amount <= 64 && name.starts_with("minecraft:"),
                "native item identity/count invalid"
            );
            slots[slot] =
                json!({"slot":slot,"name":name,"count":amount,"components":item["components"]});
        }
        let name = probe["name"]
            .as_str()
            .context("native container name missing")?;
        ensure!(
            !result.contains_key(name),
            "duplicate native container probe name"
        );
        result.insert(
            name.to_owned(),
            json!({"name":name,"position":position,
            "final":slots,"nbt":block,"source":"server read-only native export"}),
        );
    }
    Ok(Value::Object(result))
}

fn nbt_json(tag: &Tag) -> Result<Value> {
    Ok(match tag {
        Tag::Byte(v) => json!(v),
        Tag::Short(v) => json!(v),
        Tag::Int(v) => json!(v),
        Tag::Long(v) => json!(v),
        Tag::Float(v) => {
            ensure!(v.is_finite(), "nonfinite native NBT float");
            json!(v)
        }
        Tag::Double(v) => {
            ensure!(v.is_finite(), "nonfinite native NBT double");
            json!(v)
        }
        Tag::ByteArray(v) => json!(v),
        Tag::String(v) => json!(v),
        Tag::IntArray(v) => json!(v),
        Tag::LongArray(v) => json!(v),
        Tag::List(v) => Value::Array(v.elements.iter().map(nbt_json).collect::<Result<_>>()?),
        Tag::Compound(fields) => {
            let mut object = Map::new();
            for (name, value) in fields {
                ensure!(
                    !object.contains_key(name),
                    "duplicate native NBT field {name}"
                );
                object.insert(name.clone(), nbt_json(value)?);
            }
            Value::Object(object)
        }
    })
}

fn poi(payload: &Value, name: &str) -> Option<Value> {
    let value = &payload["Brain"]["memories"][name]["value"];
    let position = value["pos"].as_array()?;
    if value["dimension"].as_str() != Some("minecraft:overworld")
        || position.len() != 3
        || position.iter().any(|v| v.as_i64().is_none())
    {
        return None;
    }
    Some(Value::Array(position.clone()))
}

fn native_compound(bytes: &[u8]) -> Result<Value> {
    let mut input = bytes;
    let (_, tag) = mc_nbt::read_named(&mut input)?;
    ensure!(
        input.is_empty() && matches!(&tag, Tag::Compound(_)),
        "native domain requires one complete named NBT compound"
    );
    nbt_json(&tag)
}

pub(super) fn native_control_snapshot(world: &Path, spec: &Value) -> Result<Value> {
    let seed = spec["native_seed"]
        .as_i64()
        .context("native snapshot requires native_seed")?;
    let revision = spec["native_worldgen_revision"]
        .as_u64()
        .context("native snapshot requires explicit native_worldgen_revision")?;
    // Native format identity contract; opening validates it against the database.
    let mut identity = [0_u8; 32];
    identity[..16].copy_from_slice(b"SolarisV2Native\0");
    identity[16..24].copy_from_slice(&seed.to_be_bytes());
    identity[24..].copy_from_slice(&revision.to_be_bytes());
    let path = world.join("native/state.redb");
    let store = NativeReadOnlyStore::open_with_cache(&path, identity, 16 * 1024 * 1024)?;
    let reader = store.checkpoint_reader()?;
    let image = reader.read_checkpoint()?;
    let mut globals = Map::new();
    for (name, record) in &image.globals {
        let payload = match name.as_str() {
            "scoreboard" | "level_metadata" | "conversion_extras" => {
                native_compound(&record.payload)
            }
            _ => serde_json::from_slice::<Value>(&record.payload).map_err(Into::into),
        }
        .with_context(|| format!("decode native global {name}"))?;
        globals.insert(
            name.clone(),
            json!({"version":record.version,
            "captured_tick":record.captured_tick,
            "payload":payload}),
        );
    }
    let mut entities = Vec::with_capacity(image.entities.len());
    let mut villagers = Vec::new();
    for entity in &image.entities {
        let uuid = uuid::Uuid::from_bytes(entity.identity).to_string();
        let payload = entity.payload.as_ref().map(nbt_json).transpose()?;
        if let Some(payload) = &payload
            && payload["id"].as_str() == Some("minecraft:villager")
        {
            let home = poi(payload, "minecraft:home");
            let job_site = poi(payload, "minecraft:job_site");
            let profession = payload["VillagerData"]["profession"].clone();
            let level = payload["VillagerData"]["level"].as_i64();
            let brain_valid = home.is_some()
                && job_site.is_some()
                && level.is_some_and(|v| (1..=5).contains(&v))
                && profession.as_str().is_some();
            villagers.push(json!({"uuid":uuid,"profession":profession,
                    "home":home,"job_site":job_site,"brain_valid":brain_valid,
                    "brain":payload["Brain"],"inventory":payload["Inventory"],
                    "offers":payload["Offers"]}));
        }
        entities.push(json!({"uuid":uuid,"dimension":entity.dimension,
            "chunk":[entity.position.x,entity.position.z],"journal_lsn":entity.journal_lsn,
            "generated_slot":entity.generated_slot,"active":payload.is_some(),"payload":payload}));
    }
    let players = image
        .players
        .iter()
        .map(|player| -> Result<Value> {
            Ok(
                json!({"uuid":uuid::Uuid::from_bytes(player.uuid).to_string(),
            "version":player.record.version,"captured_tick":player.record.captured_tick,
            "payload":native_compound(&player.record.payload).context("decode native player")?}),
            )
        })
        .collect::<Result<Vec<_>>>()?;
    let mut chunks = Vec::new();
    reader.visit_chunk_positions(|dimension, position| {
        chunks.push(json!({"dimension":dimension,"position":[position.x,position.z]}));
        Ok(())
    })?;
    let mut provenance = Vec::new();
    if let Some(blocks) = spec["blocks"].as_array() {
        for block in blocks {
            let position = block["position"]
                .as_array()
                .context("block probe position missing")?;
            ensure!(
                position.len() == 3,
                "native block position needs three coordinates"
            );
            let coordinate = |i: usize| -> Result<i32> {
                Ok(i32::try_from(
                    position[i].as_i64().context("noninteger block position")?,
                )?)
            };
            let origin = store.block_origin(
                0,
                mc_world::BlockPos {
                    x: coordinate(0)?,
                    y: coordinate(1)?,
                    z: coordinate(2)?,
                },
            )?;
            provenance.push(json!({"name":block["name"],"position":position,
                "origin":origin.origin,"native_state_id":origin.native_state_id,
                "applied_lsn":origin.applied_lsn}));
        }
    }
    let mut exported = BTreeMap::new();
    let blocks = exported_blocks(spec, &mut exported)?;
    let containers = exported_containers(spec, &mut exported)?;
    Ok(json!({"schema_version":1,"read_only":true,"world":world,
        "identity":identity,"checkpoint_tick":image.tick,"globals":globals,
        "entities":entities,"villagers":villagers,"players":players,"containers":containers,"blocks":blocks,
        "persisted_chunks":chunks,"block_provenance":provenance,
        "block_provenance_scope":"native dictionary ids, not runtime protocol state ids; no fabricated procedural base"}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn native_json_does_not_collapse_duplicate_owner_fields() {
        let tag = Tag::Compound(vec![
            ("id".into(), Tag::String("minecraft:cow".into())),
            ("id".into(), Tag::String("minecraft:pig".into())),
        ]);
        assert!(nbt_json(&tag).is_err());
        assert!(nbt_json(&Tag::Double(f64::NAN)).is_err());
    }

    #[test]
    fn native_compound_decodes_committed_global_and_player_records() {
        let tag = Tag::Compound(vec![
            ("Health".into(), Tag::Float(20.0)),
            (
                "Pos".into(),
                Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::DOUBLE,
                    elements: vec![Tag::Double(1.0), Tag::Double(64.0), Tag::Double(2.0)],
                }),
            ),
        ]);
        let mut bytes = Vec::new();
        mc_nbt::write_named(&mut bytes, "", &tag).unwrap();
        assert_eq!(
            native_compound(&bytes).unwrap(),
            json!({"Health":20.0,"Pos":[1.0,64.0,2.0]})
        );
        bytes.push(0);
        assert!(native_compound(&bytes).is_err());
        assert!(native_compound(b"{}").is_err());
        assert!(native_compound(&[mc_nbt::tag_type::INT, 0, 0, 0, 0, 0, 1]).is_err());
    }

    #[test]
    fn native_poi_requires_real_dimension_and_integer_position() {
        let mut payload = json!({"Brain":{"memories":{"minecraft:home":{
            "value":{"dimension":"minecraft:overworld","pos":[1,64,2]}}}}});
        assert_eq!(poi(&payload, "minecraft:home"), Some(json!([1, 64, 2])));
        payload["Brain"]["memories"]["minecraft:home"]["value"]["pos"] = json!([1, 64.5, 2]);
        assert!(poi(&payload, "minecraft:home").is_none());
        payload["Brain"]["memories"]["minecraft:home"]["value"]["pos"] = json!([1, 64, 2]);
        payload["Brain"]["memories"]["minecraft:home"]["value"]["dimension"] =
            json!("minecraft:the_nether");
        assert!(poi(&payload, "minecraft:home").is_none());
    }

    #[test]
    fn readonly_export_decoder_checks_actual_region_chunk_identity() {
        let directory = tempfile::tempdir().unwrap();
        std::fs::create_dir(directory.path().join("region")).unwrap();
        let mut payload = Vec::new();
        mc_nbt::write_named_document(
            &mut payload,
            "",
            &Tag::Compound(vec![
                ("xPos".into(), Tag::Int(0)),
                ("zPos".into(), Tag::Int(0)),
            ]),
        )
        .unwrap();
        let mut bytes = vec![0_u8; 3 * 4096];
        bytes[..4].copy_from_slice(&[0, 0, 2, 1]);
        bytes[8192..8196].copy_from_slice(&u32::try_from(payload.len() + 1).unwrap().to_be_bytes());
        bytes[8196] = 3;
        bytes[8197..8197 + payload.len()].copy_from_slice(&payload);
        let path = directory.path().join("region/r.0.0.mca");
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        assert_eq!(
            exported_chunk(directory.path(), 0, 0).unwrap()["xPos"],
            json!(0)
        );
        // The next location is absent; no synthetic empty chunk is substituted.
        assert!(exported_chunk(directory.path(), 1, 0).is_err());
        bytes[0..4].copy_from_slice(&[0, 0, 3, 1]);
        std::fs::File::create(&path)
            .unwrap()
            .write_all(&bytes)
            .unwrap();
        assert!(exported_chunk(directory.path(), 0, 0).is_err());
    }
}
