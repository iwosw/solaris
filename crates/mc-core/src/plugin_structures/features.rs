use super::*;

const CELL_CHUNKS: i32 = 32;
fn feature_id(state: &TickState, site: &mc_world::GeneratedVillageSite) -> String {
    let digest = inventories::hash(&(state.world.seed(), state.dimension.name(), site.start_chunk));
    format!(
        "village_{}_{}_{}",
        site.start_chunk.0,
        site.start_chunk.1,
        &digest[..8]
    )
}
fn start_chunk(state: &TickState, id: &str) -> Option<(i32, i32)> {
    let mut parts = id.strip_prefix("village_")?.split('_');
    let x = parts.next()?.parse().ok()?;
    let z = parts.next()?.parse().ok()?;
    let digest = parts.next()?;
    if parts.next().is_some() || digest.len() != 8 {
        return None;
    }
    let expected = inventories::hash(&(state.world.seed(), state.dimension.name(), (x, z)));
    (digest == &expected[..8]).then_some((x, z))
}
fn interesting(name: &str) -> bool {
    inventories::container_name(name)
        || name.ends_with("_bed")
        || name == "minecraft:bell"
        || matches!(
            name,
            "minecraft:crafting_table"
                | "minecraft:campfire"
                | "minecraft:soul_campfire"
                | "minecraft:composter"
                | "minecraft:cartography_table"
                | "minecraft:fletching_table"
                | "minecraft:lectern"
                | "minecraft:stonecutter"
                | "minecraft:loom"
                | "minecraft:smithing_table"
                | "minecraft:grindstone"
                | "minecraft:cauldron"
                | "minecraft:water_cauldron"
                | "minecraft:lava_cauldron"
                | "minecraft:powder_snow_cauldron"
        )
}
fn snapshot(
    state: &TickState,
    site: &mc_world::GeneratedVillageSite,
) -> Result<ScriptWorldFeature, Failure> {
    let bounds = ScriptWorldBounds::new(
        [site.min.x, site.min.y, site.min.z],
        [site.max.x, site.max.y, site.max.z],
    )
    .map_err(|_| Failure::Capacity)?;
    let id = feature_id(state, site);
    let mut known = true;
    for z in site.min.z.div_euclid(16)..=site.max.z.div_euclid(16) {
        for x in site.min.x.div_euclid(16)..=site.max.x.div_euclid(16) {
            known &= state.world.has_chunk(x, z);
        }
    }
    if !known {
        return Ok(ScriptWorldFeature::new(
            id,
            "minecraft:village".to_owned(),
            state.dimension.name().to_owned(),
            bounds,
            false,
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ));
    }
    let mut blocks = Vec::new();
    let mut points = Vec::new();
    for z in site.min.z..=site.max.z {
        for x in site.min.x..=site.max.x {
            for y in site.min.y..=site.max.y {
                let state_id = state.world.block_at(x, y, z).ok_or(Failure::Unloaded)?;
                let definition = state
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(state_id))
                    .ok_or(Failure::InvalidRequest)?;
                let name = definition.block.id.as_str();
                if !interesting(name) {
                    continue;
                }
                let at = [x, y, z];
                // Multi-block interactions expose only the canonical anchor half.
                if name.ends_with("_bed")
                    && !definition
                        .properties
                        .iter()
                        .any(|(k, v)| k == "part" && v == "head")
                {
                    continue;
                }
                if name.ends_with("chest")
                    && definition
                        .properties
                        .iter()
                        .any(|(k, v)| k == "type" && v == "left")
                {
                    continue;
                }
                blocks.push(ScriptWorldFeatureBlock::new(at, name.to_owned()));
                if inventories::container_name(name)
                    && state.block_entities.contains_key(&(x, y, z))
                {
                    points.push(ScriptWorksiteInteractionPoint::Container { at });
                } else if name.ends_with("_bed") || name == "minecraft:bell" {
                    points.push(ScriptWorksiteInteractionPoint::Block {
                        at,
                        block: name.to_owned(),
                    });
                } else {
                    points.push(ScriptWorksiteInteractionPoint::Workstation {
                        at,
                        capability: name.to_owned(),
                    });
                }
                if blocks.len() > MAX_WORLD_FEATURE_BLOCKS || points.len() > MAX_WORKSITE_POINTS {
                    return Err(Failure::Capacity);
                }
            }
        }
    }
    let mut entities = BTreeSet::new();
    for z in site.min.z.div_euclid(16)..=site.max.z.div_euclid(16) {
        for x in site.min.x.div_euclid(16)..=site.max.x.div_euclid(16) {
            let chunk = state
                .world
                .chunk(crate::boundary::ChunkPos { x, z })
                .ok_or(Failure::Unloaded)?;
            for marker in chunk.generated_entities() {
                let at = marker.position.map(|v| v.floor() as i32);
                if (0..3).all(|i| (bounds.min[i]..=bounds.max[i]).contains(&at[i])) {
                    entities.insert(crate::villager::generated_uuid(&marker.claim).to_string());
                }
            }
        }
    }
    if entities.len() > MAX_WORLD_FEATURE_ENTITIES {
        return Err(Failure::Capacity);
    }
    let mut feature = ScriptWorldFeature::new(
        id,
        "minecraft:village".to_owned(),
        state.dimension.name().to_owned(),
        bounds,
        true,
        blocks,
        points,
        entities.into_iter().collect(),
    );
    feature.canonicalize();
    feature.validate().map_err(|_| Failure::InvalidRequest)?;
    Ok(feature)
}
pub(super) fn query(
    state: &TickState,
    id: &str,
) -> Result<(u64, ScriptWorldStructureResult), Failure> {
    let target = world(state, "minecraft:overworld")?;
    let start = start_chunk(target, id).ok_or(Failure::NotFound)?;
    let site = target
        .world
        .village_sites_in_region(start, start)
        .into_iter()
        .find(|s| s.start_chunk == start)
        .ok_or(Failure::NotFound)?;
    Ok((
        target.world.revision_now(),
        ScriptWorldStructureResult::Feature {
            feature: Box::new(snapshot(target, &site)?),
        },
    ))
}
pub(super) fn list(
    state: &TickState,
    cursor: Option<&str>,
    limit: u8,
) -> Result<(u64, ScriptWorldStructureResult), Failure> {
    let target = world(state, "minecraft:overworld")?;
    let start = if let Some(cursor) = cursor {
        let (x, z) = cursor.split_once(':').ok_or(Failure::CursorExpired)?;
        [
            x.parse::<i32>().map_err(|_| Failure::CursorExpired)?,
            z.parse::<i32>().map_err(|_| Failure::CursorExpired)?,
        ]
    } else {
        let spawn = target.world.world_spawn();
        [spawn[0].div_euclid(512), spawn[2].div_euclid(512)]
    };
    let mut features = Vec::new();
    let mut next = start;
    for index in 0..usize::from(limit) {
        let cell = [
            start[0]
                .checked_add(index as i32)
                .ok_or(Failure::CursorExpired)?,
            start[1],
        ];
        let min = (
            cell[0]
                .checked_mul(CELL_CHUNKS)
                .ok_or(Failure::CursorExpired)?,
            cell[1]
                .checked_mul(CELL_CHUNKS)
                .ok_or(Failure::CursorExpired)?,
        );
        let max = (
            min.0
                .checked_add(CELL_CHUNKS - 1)
                .ok_or(Failure::CursorExpired)?,
            min.1
                .checked_add(CELL_CHUNKS - 1)
                .ok_or(Failure::CursorExpired)?,
        );
        let sites = target.world.village_sites_in_region(min, max);
        if features.len() + sites.len() > MAX_WORLD_FEATURE_PAGE {
            if features.is_empty() {
                return Err(Failure::Capacity);
            }
            next = cell;
            break;
        }
        for site in sites {
            features.push(snapshot(target, &site)?);
        }
        next = [
            cell[0].checked_add(1).ok_or(Failure::CursorExpired)?,
            cell[1],
        ];
    }
    let mut page = ScriptWorldFeaturePage::new(features, Some(format!("{}:{}", next[0], next[1])));
    page.canonicalize();
    page.validate().map_err(|_| Failure::InvalidRequest)?;
    Ok((
        target.world.revision_now(),
        ScriptWorldStructureResult::Features {
            page: Box::new(page),
        },
    ))
}
