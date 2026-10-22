//! Solaris side of the quirk/farm parity gate (`tools/harness/quirk_parity`).
//!
//! Replays the scenario JSON files named by `SOLARIS_QUIRK_SCENARIOS`
//! (colon-separated paths) against the real native tick in-process, using the
//! same console command strings the vanilla oracle receives, and writes the
//! observed snapshots to `SOLARIS_QUIRK_RECEIPT`. The test is `#[ignore]`d:
//! the harness profile owns when it runs and compares it with vanilla.

use std::sync::Arc;

use serde_json::{Value, json};

use crate::boundary::{ChangeSet, ChunkPos, PlayerChange};
use crate::command_console::{CONNECTION, execute};
use crate::entities::EntityKind;
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;
use mc_world::BlockStateId;

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            BlockStateId(self.0),
            mc_data::Identifier::parse("minecraft:the_void").unwrap(),
        )
    }
}

fn int_array<const N: usize>(value: &Value, what: &str) -> [i32; N] {
    let items = value.as_array().unwrap_or_else(|| panic!("{what}: array"));
    assert_eq!(items.len(), N, "{what}: length");
    let mut out = [0; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = i32::try_from(item.as_i64().unwrap_or_else(|| panic!("{what}: int")))
            .unwrap_or_else(|_| panic!("{what}: range"));
    }
    out
}

fn fixture(boxes: &[[i32; 6]], seed: i64) -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let air = registry.states.air;
    let world = ChunkWorld::new(
        seed,
        Arc::new(Empty(air)),
        &registry,
        BlockStateId(air),
        [0, 200, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 4096);
    // One forced ticket per observed box covering all its chunks, like the
    // vanilla driver's `forceload add` over the same rectangles.
    for (index, box_) in boxes.iter().enumerate() {
        let (min_x, max_x) = (box_[0].div_euclid(16), box_[3].div_euclid(16));
        let (min_z, max_z) = (box_[2].div_euclid(16), box_[5].div_euclid(16));
        let centre = ChunkPos {
            x: (min_x + max_x) / 2,
            z: (min_z + max_z) / 2,
        };
        let radius = ((max_x - min_x).max(max_z - min_z) + 1) / 2;
        state
            .tickets
            .insert(format!("quirk-parity-{index}"), (centre, radius));
    }
    state.refresh_active_chunks(state.tick);
    state.world.ensure_blocking(&state.desired_chunks());
    state.world.fold_overlay();
    state
}

fn block_string(state: &TickState, id: u32) -> String {
    let block = state.registry.blocks.by_id(BlockStateId(id)).unwrap();
    let mut text = block.block.id.to_string();
    if !block.properties.is_empty() {
        let mut properties = block.properties.clone();
        properties.sort();
        let body: Vec<String> = properties.iter().map(|(k, v)| format!("{k}={v}")).collect();
        text.push_str(&format!("[{}]", body.join(",")));
    }
    text
}

fn entity_snapshot(state: &TickState, box_: [i32; 6]) -> Vec<Value> {
    let mut out = Vec::new();
    for slot in state.entities.slots() {
        let body = state.entities.body(slot);
        if !body.alive {
            continue;
        }
        let pos = state.entities.pos(slot);
        let inside = (0..3).all(|axis| {
            pos[axis] >= f64::from(box_[axis]) && pos[axis] < f64::from(box_[axis + 3]) + 1.0
        });
        if !inside {
            continue;
        }
        let row = state.entities.row(slot);
        let type_name = match body.kind {
            EntityKind::Player { .. } => "minecraft:player".to_owned(),
            EntityKind::Mob { type_id }
            | EntityKind::Item { type_id }
            | EntityKind::ExperienceOrb { type_id } => {
                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                    .map_or_else(|| format!("unknown:{type_id}"), |c| c.name.to_owned())
            }
        };
        let mut entry = json!({
            "type": type_name,
            "pos": pos,
            "motion": state.entities.vel(slot),
            "health": row.health,
        });
        if let Some(item) = &row.item {
            let name = state
                .registry
                .items
                .name_of(item.item_id)
                .map_or_else(|| format!("unknown:{}", item.item_id), ToString::to_string);
            entry["item"] = json!(name);
            entry["count"] = json!(item.count);
        }
        if let Some(tnt) = &row.tnt {
            entry["fuse"] = json!(tnt.fuse);
        }
        if let Some(falling) = &row.falling {
            let _ = falling;
        }
        entry["age"] = json!(row.age);
        out.push(entry);
    }
    out
}

fn snapshot(state: &TickState, boxes: &[[i32; 6]]) -> Value {
    let mut blocks = serde_json::Map::new();
    let mut containers = serde_json::Map::new();
    let mut entities = Vec::new();
    let mut cells = Vec::new();
    for box_ in boxes {
        let cell = snapshot_box(state, *box_);
        if let Some(found) = cell["blocks"].as_object() {
            blocks.extend(found.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        if let Some(found) = cell["containers"].as_object() {
            containers.extend(found.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
        if let Some(found) = cell["entities"].as_array() {
            entities.extend(found.iter().cloned());
        }
        cells.push(cell);
    }
    let mut out = json!({ "blocks": blocks, "entities": entities, "containers": containers });
    if boxes.len() > 1 {
        out["cells"] = json!(cells);
    }
    out
}

fn snapshot_box(state: &TickState, box_: [i32; 6]) -> Value {
    let mut blocks = serde_json::Map::new();
    for y in box_[1]..=box_[4] {
        for z in box_[2]..=box_[5] {
            for x in box_[0]..=box_[3] {
                let Some(id) = state.world.block_at(x, y, z) else {
                    continue;
                };
                if id == state.registry.states.air {
                    continue;
                }
                let text = block_string(state, id);
                if matches!(
                    text.as_str(),
                    "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"
                ) {
                    continue;
                }
                blocks.insert(format!("{x},{y},{z}"), json!(text));
            }
        }
    }
    // Item totals per container block entity (hoppers, chests, ...): the farm
    // gates read production from here.
    let mut containers = serde_json::Map::new();
    for ((x, y, z), entity) in state.block_entities.iter() {
        let inside = (box_[0]..=box_[3]).contains(x)
            && (box_[1]..=box_[4]).contains(y)
            && (box_[2]..=box_[5]).contains(z);
        if !inside {
            continue;
        }
        let mut totals = std::collections::BTreeMap::<String, i64>::new();
        for stack in entity.slots.iter().filter(|stack| stack.count > 0) {
            let name = state
                .registry
                .items
                .name_of(stack.item_id)
                .map_or_else(|| format!("unknown:{}", stack.item_id), ToString::to_string);
            *totals.entry(name).or_default() += i64::from(stack.count);
        }
        if !totals.is_empty() {
            containers.insert(format!("{x},{y},{z}"), json!(totals));
        }
    }
    json!({
        "blocks": blocks,
        "entities": entity_snapshot(state, box_),
        "containers": containers,
    })
}

fn commands(state: &mut TickState, lines: &[Value], log: &mut Vec<String>) {
    for line in lines {
        let line = line.as_str().expect("command string");
        let mut changes = ChangeSet::default();
        execute(state, CONNECTION, line, 4, &mut changes);
        state.world.fold_overlay();
        for change in state.outbox.remove(&CONNECTION).unwrap_or_default() {
            if let PlayerChange::Chat { message } = change {
                log.push(format!("{line} => {message}"));
            }
        }
    }
}

fn run_scenario(scenario: &Value) -> Value {
    let id = scenario["id"].as_str().expect("scenario id");
    let boxes: Vec<[i32; 6]> = if let Some(list) = scenario["boxes"].as_array() {
        list.iter().map(|b| int_array(b, "boxes")).collect()
    } else {
        vec![int_array(&scenario["box"], "box")]
    };
    let started = std::time::Instant::now();
    let mut state = fixture(&boxes, scenario["seed"].as_i64().unwrap_or(1));
    // Solaris draws are a pure function of (entity id, tick, position), not of the world
    // seed: without this every `seeds` variant would replay the identical sample, so a
    // rate scenario would compare vanilla's distribution against a single draw. Burning a
    // seed-dependent run of wire ids gives each variant an independent sample.
    for _ in 0..scenario["seed"].as_i64().unwrap_or(0).rem_euclid(9973) * 11 {
        let _ = state.entities.alloc_id();
    }
    let mut log = Vec::new();
    let empty = Vec::new();
    commands(
        &mut state,
        scenario["setup"].as_array().unwrap_or(&empty),
        &mut log,
    );
    // Block updates of the setup commands are processed by the first tick; settle them
    // before the first phase exactly as the vanilla driver does (vanilla applies them
    // immediately inside the command).
    for _ in 0..scenario["settle_ticks"].as_u64().unwrap_or(1) {
        let _ = run_tick(&mut state, &[]);
        state.outbox.clear();
    }
    let mut phases = vec![json!({
        "name": "setup", "tick": state.tick, "snapshot": snapshot(&state, &boxes),
    })];
    let trace = scenario["trace"].as_bool().unwrap_or(false);
    for phase in scenario["phases"].as_array().unwrap_or(&empty) {
        commands(
            &mut state,
            phase["commands"].as_array().unwrap_or(&empty),
            &mut log,
        );
        let ticks = phase["ticks"].as_u64().unwrap_or(0);
        let mut trace_rows = Vec::new();
        for _ in 0..ticks {
            let _ = run_tick(&mut state, &[]);
            state.outbox.clear();
            if trace {
                trace_rows.push(snapshot(&state, &boxes));
            }
        }
        let mut entry = json!({
            "name": phase["name"], "tick": state.tick, "snapshot": snapshot(&state, &boxes),
        });
        if trace {
            entry["trace"] = json!(trace_rows);
        }
        phases.push(entry);
    }
    json!({
        "id": id,
        "server": "solaris",
        "phases": phases,
        "command_log": log,
        "wall_ms": started.elapsed().as_millis() as u64,
    })
}

#[test]
#[ignore = "driven by tools/harness/quirk_parity via SOLARIS_QUIRK_SCENARIOS"]
fn quirk_parity_solaris_replay() {
    let scenarios = std::env::var("SOLARIS_QUIRK_SCENARIOS").expect("SOLARIS_QUIRK_SCENARIOS");
    let receipt = std::env::var("SOLARIS_QUIRK_RECEIPT").expect("SOLARIS_QUIRK_RECEIPT");
    let mut results = Vec::new();
    for path in scenarios.split(':').filter(|p| !p.is_empty()) {
        let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let scenario: Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
        results.push(run_scenario(&scenario));
    }
    std::fs::write(&receipt, serde_json::to_vec_pretty(&results).unwrap()).unwrap();
    println!("SOLARIS_QUIRK_REPLAY_OK {receipt}");
}
