use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

mod ownership_rules;

use ownership_rules::{LOWER_CRATE_OWNERSHIP, OwnershipRule};

const PLUGIN_API_CRATES: &[&str] = &["mc-script"];
const FORBIDDEN_API_TYPES: &[&str] = &["EntityStore"];
const FORBIDDEN_API_TRANSPORTS: &[&str] = &[
    "TryRecvError",
    "std::sync::mpsc",
    "tokio::sync",
    "mpsc::Sender",
    "mpsc::Receiver",
    "SyncSender",
    "Receiver<",
    "Sender<",
    "Arc<Mutex",
    "Arc<RwLock",
    "parking_lot",
    "DashMap",
    "JoinHandle",
];
const FORBIDDEN_API_DEPENDENCIES: &[&str] = &[
    "mc-world",
    "mc-physics",
    "mc-data",
    "mc-protocol",
    "mc-nbt",
    "mc-worldgen",
];

#[derive(Debug, PartialEq, Eq)]
struct Finding {
    path: PathBuf,
    line: usize,
    message: String,
}

/// Placement rules `mc-data` must own.
const BLOCK_PLACEMENT_SEMANTICS: &[&str] = &[
    "resolve_stair_shape",
    "merge_slab_state",
    "apply_waterlogged_state",
    "torch_state_for_direction",
    "sign_state_for_direction",
];

pub(super) fn run_code_health() -> Result<(), i32> {
    let root = workspace_root().map_err(|err| {
        eprintln!("code-health: {err}");
        2
    })?;
    let mut findings = Vec::new();
    scan_ownership(&root, LOWER_CRATE_OWNERSHIP, &mut findings);
    scan_block_placement_semantics(&root, &mut findings);
    scan_rust_sources(&root.join("crates"), &mut findings);
    scan_harness_catalog_ownership(&root, &mut findings);
    scan_api_manifests(&root, &mut findings);

    println!("Solaris code-health report");
    println!();

    if findings.is_empty() {
        println!("summary: 0 fail");
        println!("verdict: KEEP");
        return Ok(());
    }

    for finding in &findings {
        println!(
            "FAIL: {}:{}: {}",
            display_path(&root, &finding.path),
            finding.line,
            finding.message
        );
    }

    println!();
    println!("summary: {} fail", findings.len());
    println!("verdict: CLEANUP_REQUIRED");
    Err(1)
}

fn workspace_root() -> Result<PathBuf, String> {
    let mut dir = env::current_dir().map_err(|err| format!("current_dir failed: {err}"))?;
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err("could not find workspace root with Cargo.toml and crates/".into());
        }
    }
}

fn scan_rust_sources(dir: &Path, findings: &mut Vec<Finding>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            scan_rust_sources(&path, findings);
        } else if file_type.is_file() && path.extension().is_some_and(|ext| ext == "rs") {
            scan_rust_file(&path, findings);
        }
    }
}

fn scan_ownership(root: &Path, rules: &[OwnershipRule], findings: &mut Vec<Finding>) {
    for rule in rules {
        let module_path = root.join(rule.module_file);
        let module_source = fs::read_to_string(&module_path).ok();
        if module_source.is_none() {
            findings.push(Finding {
                path: module_path.clone(),
                line: 1,
                message: format!("{} owner module file is missing", rule.name),
            });
        }

        let parent_path = root.join(rule.parent_file);
        if let Ok(parent_source) = fs::read_to_string(&parent_path) {
            if !parent_source
                .lines()
                .any(|line| line.trim() == rule.mod_declaration)
            {
                findings.push(Finding {
                    path: parent_path.clone(),
                    line: 1,
                    message: format!(
                        "{} parent module is missing `{}`",
                        rule.name, rule.mod_declaration
                    ),
                });
            }
            if let Some(line) = source_line(&parent_source, rule.definition_anchor) {
                findings.push(Finding {
                    path: parent_path.clone(),
                    line,
                    message: format!("{} definition returned to a parent module", rule.name),
                });
            }
        } else {
            findings.push(Finding {
                path: parent_path.clone(),
                line: 1,
                message: format!("{} parent module file is missing", rule.name),
            });
        }

        let definition_path = root.join(rule.definition_file);
        let separate_definition_source = if definition_path == module_path {
            None
        } else {
            match fs::read_to_string(&definition_path) {
                Ok(source) => Some(source),
                Err(_) => {
                    findings.push(Finding {
                        path: definition_path.clone(),
                        line: 1,
                        message: format!("{} definition file is missing", rule.name),
                    });
                    None
                }
            }
        };
        let definition_source = if definition_path == module_path {
            module_source.as_deref()
        } else {
            separate_definition_source.as_deref()
        };
        if definition_source.is_some_and(|source| !source.contains(rule.definition_anchor)) {
            findings.push(Finding {
                path: definition_path,
                line: 1,
                message: format!(
                    "{} owner module is missing definition anchor `{}`",
                    rule.name, rule.definition_anchor
                ),
            });
        }
    }
}

fn source_line(source: &str, anchor: &str) -> Option<usize> {
    source.find(anchor).map(|offset| {
        source[..offset]
            .bytes()
            .filter(|byte| *byte == b'\n')
            .count()
            + 1
    })
}

fn scan_block_placement_semantics(root: &Path, findings: &mut Vec<Finding>) {
    let owner_path = root.join("crates/mc-data/src/block_placement_26_1_2.rs");
    let owner_source = match fs::read_to_string(&owner_path) {
        Ok(source) => source,
        Err(_) => {
            findings.push(Finding {
                path: owner_path,
                line: 1,
                message: "block placement semantics owner module is missing".into(),
            });
            return;
        }
    };
    for required in BLOCK_PLACEMENT_SEMANTICS {
        if !owner_source.contains(required) {
            findings.push(Finding {
                path: owner_path.clone(),
                line: 1,
                message: format!("block placement semantics owner is missing `{required}`"),
            });
        }
    }
}

fn scan_rust_file(path: &Path, findings: &mut Vec<Finding>) {
    let Ok(source) = fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = source.lines().collect();
    scan_generic_modules(path, &lines, findings);
    scan_plant_rules_boundary(path, &lines, findings);
    scan_movement_rules_boundary(path, &lines, findings);
    scan_domain_value_boundary(path, &lines, findings);
    scan_item_stack_boundary(path, &lines, findings);
    scan_inventory_semantics_boundary(path, &lines, findings);
    scan_recipe_semantics_boundary(path, &lines, findings);
    scan_block_semantics_boundary(path, &lines, findings);
    scan_player_combat_boundary(path, &lines, findings);
    scan_survival_mining_boundary(path, &lines, findings);
    scan_lower_crate_transport_leaks(path, &lines, findings);
    scan_domain_type_ownership(path, &lines, findings);
    scan_api_leaks(path, &lines, findings);
}

fn scan_plant_rules_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    if !path.ends_with("mc-world/src/plant_rules_26_1_2.rs") {
        return;
    }

    const FORBIDDEN: &[&str] = &[
        "crate::play",
        "mc_protocol",
        "ItemRegistry",
        "protocol_id",
        "item_id",
        ".id_of(",
        ".name_of(",
        "SessionRegistry",
        "OutboundCommand",
        "VisibilityDispatch",
        "WorldMutationView",
        "WorldHandle",
        "WorldWriter",
        "WorldWrite",
        "Mutex",
        "RwLock",
        "parking_lot",
        ".lock(",
        "asyncfn",
        ".await",
        "tokio::",
        "mpsc",
        "Sender<",
        "Receiver<",
        "write_packet",
        "Clientbound",
        "Serverbound",
    ];
    for (index, line) in lines.iter().enumerate() {
        let normalized = line.split_whitespace().collect::<String>();
        if FORBIDDEN
            .iter()
            .any(|forbidden| normalized.contains(forbidden))
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plant rules depend on network, session, mutation, or async runtime"
                    .into(),
            });
        }
    }
}

fn scan_movement_rules_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let source = lines.join("\n");
    if path.ends_with("mc-world/src/chunk.rs") {
        if !source.contains("pub fn chunk_rectangle_for_world_bounds(") {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "bounded world chunk coverage rule left mc-world".into(),
            });
        }
        return;
    }
    if path.ends_with("mc-physics/src/lib.rs") {
        if !source.contains("pub fn displacement_within_limit(") {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "movement displacement budget rule left mc-physics".into(),
            });
        }
        if !source.contains("pub fn authoritative_pose_within_limits(") {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "authoritative pose validation rule left mc-physics".into(),
            });
        }
        if !source.contains("pub fn sweep_sample_count(") {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "movement sweep sampling rule left mc-physics".into(),
            });
        }
        if !source.contains("pub fn resolve_collision_displacement")
            || !source.contains("pub fn horizontal_movement_residual_within_limit(")
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "movement collision-resolution rule left mc-physics".into(),
            });
        }
        if !source.contains("pub fn rotation_is_finite(")
            || !source.contains("pub fn clamp_world_position(")
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "movement normalization rules left mc-physics".into(),
            });
        }
        if !source.contains("pub fn aabb_intersects_deflated_obstacle(") {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "movement AABB overlap rule left mc-physics".into(),
            });
        }
        if !source.contains("pub fn next_fall_start_y(") {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "player fall-state rule left mc-physics".into(),
            });
        }
    }
}

fn scan_domain_value_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let source = lines.join("\n");
    if path.ends_with("mc-domain/src/lib.rs") {
        for anchor in [
            "pub enum GameMode",
            "pub enum Direction",
            "pub enum InteractionHand",
        ] {
            if !source.contains(anchor) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: 1,
                    message: "shared gameplay value type left mc-domain owner".into(),
                });
            }
        }
        return;
    }

    if !path.ends_with("mc-protocol/src/packets/play.rs") {
        return;
    }
    for anchor in [
        "pub use mc_domain::GameMode;",
        "pub use mc_domain::Direction;",
        "pub use mc_domain::InteractionHand;",
    ] {
        if !source.contains(anchor) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "mc-protocol stopped adapting shared gameplay values from mc-domain"
                    .into(),
            });
        }
    }
}

fn scan_item_stack_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let source = lines.join("\n");
    if path.ends_with("mc-data/src/item_stack.rs") {
        if !source.contains("pub struct ItemStack")
            || !source.contains("pub const EMPTY: ItemStack")
            || !source.contains("pub fn is_empty(&self)")
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "ItemStack domain value left mc-data owner".into(),
            });
        }
        return;
    }

    if path.ends_with("mc-protocol/src/packets/play.rs")
        && !source.contains("pub use mc_data::item_stack::ItemStack;")
    {
        findings.push(Finding {
            path: path.to_path_buf(),
            line: 1,
            message: "mc-protocol stopped adapting canonical ItemStack from mc-data".into(),
        });
    }
}

fn scan_inventory_semantics_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let source = lines.join("\n");
    if path.ends_with("mc-data/src/inventory_semantics_26_1_2.rs") {
        for anchor in [
            "pub fn canonicalize_empty(",
            "pub fn canonical_stack(",
            "pub fn can_stack(",
            "pub fn take_from_stack(",
            "pub fn decrement_stack(",
            "pub fn take_throw_stack(",
            "pub fn apply_regular_pickup_slot(",
            "pub fn apply_regular_swap_slot(",
            "pub fn apply_regular_throw_slot(",
            "pub fn apply_outside_pickup_click(",
        ] {
            if !source.contains(anchor) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: 1,
                    message: "inventory stack transaction semantics left mc-data owner".into(),
                });
            }
        }
        return;
    }
    if path.ends_with("mc-data/src/item_semantics_26_1_2.rs")
        && !source.contains("pub fn equippable_player_slot(")
    {
        findings.push(Finding {
            path: path.to_path_buf(),
            line: 1,
            message: "equipment slot semantics left mc-data owner".into(),
        });
        return;
    }
    if path.ends_with("mc-data/src/armor.rs") {
        for anchor in [
            "pub struct ArmorStats",
            "pub fn armor_reduced_damage(",
            "pub fn protection_reduced_damage(",
        ] {
            if !source.contains(anchor) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: 1,
                    message: "armor reduction semantics left mc-data owner".into(),
                });
            }
        }
    }
}

fn scan_recipe_semantics_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let source = lines.join("\n");
    if path.ends_with("mc-data/src/recipes.rs") {
        for anchor in [
            "pub fn ingredient_accepts_item(",
            "pub fn ingredient_accepts_stack(",
            "pub fn stonecutting_recipe_accepts_input(",
            "pub enum CookingKind",
            "pub fn find_cooking_recipe_for_item(",
            "pub fn furnace_fuel_ticks(",
            "pub fn shaped_recipe_matches(",
            "pub fn shapeless_recipe_matches(",
            "pub fn crafting_recipe_matches(",
            "pub fn repair_item_crafting_result(",
            "pub fn crafting_result_from_input(",
            "pub fn furnace_experience_award(",
        ] {
            if !source.contains(anchor) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: 1,
                    message: "recipe selection semantics left mc-data owner".into(),
                });
            }
        }
    }
}

fn scan_block_semantics_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let source = lines.join("\n");
    if path.ends_with("mc-data/src/block_semantics_26_1_2.rs") {
        for anchor in [
            "pub fn passive_herd_fallback_surface_name(",
            "pub fn passable_block_name(",
        ] {
            if !source.contains(anchor) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: 1,
                    message: "26.1.2 block-name semantics left mc-data owner".into(),
                });
            }
        }
    }
}

fn scan_player_combat_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let source = lines.join("\n");

    if path.ends_with("mc-data/src/item_semantics_26_1_2.rs") {
        for anchor in [
            "pub fn is_durability_tool_path(",
            "pub fn is_mining_loot_enchantable_path(",
            "pub fn max_stack_for_stack(",
            "pub fn max_tool_damage_for_path(",
        ] {
            if !source.contains(anchor) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: 1,
                    message: "item durability semantics left mc-data owner".into(),
                });
            }
        }
        return;
    }
    if path.ends_with("mc-data/src/enchanting_26_1_2.rs") {
        for anchor in [
            "pub struct EnchantingOffer",
            "pub fn enchanting_offer(",
            "pub fn item_is_efficiency_enchantable(",
            "pub fn supported_enchantment_for_item(",
            "pub fn additional_enchantment_for_offer(",
        ] {
            if !source.contains(anchor) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: 1,
                    message: "enchanting selection semantics left mc-data owner".into(),
                });
            }
        }
    }
}

fn scan_survival_mining_boundary(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let source = lines.join("\n");

    if path.ends_with("mc-data/src/food.rs") {
        if !source.contains("pub const DEFAULT_USE_DURATION:")
            || !source.contains("pub fn rule_for_item(")
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "food duration/rule lookup left mc-data food owner".into(),
            });
        }
        return;
    }
    if path.ends_with("mc-data/src/tags.rs") {
        if !source.contains("pub fn contains_raw_id(") {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: 1,
                message: "resolved tag membership rule left mc-data TagsData".into(),
            });
        }
        return;
    }
    if path.ends_with("mc-data/src/block_mining.rs") {
        for anchor in [
            "pub fn block_break_is_denied(",
            "pub fn fallback_mining_facts(",
            "pub fn fallback_tool_suffix_for_path(",
            "pub fn fallback_tool_mining_speed(",
            "pub fn fallback_tool_allows_block_drop(",
            "pub fn destroy_progress_per_tick(",
        ] {
            if !source.contains(anchor) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: 1,
                    message: "survival mining rule left mc-data block_mining owner".into(),
                });
            }
        }
    }
}

fn scan_lower_crate_transport_leaks(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let lower_domain = [
        "/crates/mc-data/src/",
        "/crates/mc-domain/src/",
        "/crates/mc-nbt/src/",
        "/crates/mc-physics/src/",
        "/crates/mc-script/src/",
        "/crates/mc-world/src/",
        "/crates/mc-worldgen/src/",
    ]
    .iter()
    .any(|segment| normalized.contains(segment));
    if !lower_domain {
        return;
    }

    for (index, line) in lines.iter().enumerate() {
        let code = line.split("//").next().unwrap_or_default();
        for forbidden in [
            "OutboundCommand",
            "ConnectionHandle",
            "OwnedWriteHalf",
            "PacketWriter",
            "SessionRegistry",
            "VisibilityDispatch",
            "Clientbound",
            "Serverbound",
        ] {
            if code.contains(forbidden) {
                findings.push(Finding {
                    path: path.to_path_buf(),
                    line: index + 1,
                    message: format!(
                        "lower semantic crate leaks transport/session symbol `{forbidden}`"
                    ),
                });
            }
        }
    }
}

fn scan_domain_type_ownership(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let normalized = path.to_string_lossy().replace('\\', "/");
    let forbidden = ["ItemStack", "GameMode", "Direction", "InteractionHand"];

    if normalized.ends_with("/crates/mc-protocol/src/packets/play.rs") {
        for (index, line) in lines.iter().enumerate() {
            let code = line.split("//").next().unwrap_or_default().trim();
            for name in forbidden {
                if code.starts_with(&format!("pub struct {name}"))
                    || code.starts_with(&format!("pub enum {name}"))
                {
                    findings.push(Finding {
                        path: path.to_path_buf(),
                        line: index + 1,
                        message: format!(
                            "protocol layer redefines domain gameplay type `{name}` instead of adapting it"
                        ),
                    });
                }
            }
        }
    }
}

/// Scenario/profile resolution has one executable catalog: `SCENARIOS` lives
/// in `tools/harness/scenarios.py` and `PROFILES` in `tools/harness/profiles.py`.
/// This is a structural ownership rule, not a name grep: only a top-level
/// `SCENARIOS`/`PROFILES` assignment outside the owning file is a duplicate
/// authority. Selector strings, `catalog.SCENARIOS` reads, and namespaced sets
/// such as `WASM_*_SCENARIOS` or `BLOCKED_ONLY_BROAD_SCENARIOS` never match.
fn scan_harness_catalog_ownership(root: &Path, findings: &mut Vec<Finding>) {
    scan_harness_catalog_dir(&root.join("tools/harness"), findings);
}

fn scan_harness_catalog_dir(dir: &Path, findings: &mut Vec<Finding>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            scan_harness_catalog_dir(&path, findings);
            continue;
        }
        if !file_type.is_file() || path.extension().is_none_or(|ext| ext != "py") {
            continue;
        }
        let Ok(source) = fs::read_to_string(&path) else {
            continue;
        };
        let is_scenario_owner = path.ends_with("tools/harness/scenarios.py");
        let is_profile_owner = path.ends_with("tools/harness/profiles.py");
        for (index, line) in source.lines().enumerate() {
            let code = line.split('#').next().unwrap_or_default();
            let Some((name, _)) = python_top_level_assignment(code) else {
                continue;
            };
            if name == "SCENARIOS" && !is_scenario_owner {
                findings.push(Finding {
                    path: path.clone(),
                    line: index + 1,
                    message: "duplicate scenario catalog outside tools/harness/scenarios.py"
                        .to_owned(),
                });
            }
            if name == "PROFILES" && !is_profile_owner {
                findings.push(Finding {
                    path: path.clone(),
                    line: index + 1,
                    message: "duplicate profile registry outside tools/harness/profiles.py"
                        .to_owned(),
                });
            }
        }
    }
}

fn python_top_level_assignment(code: &str) -> Option<(String, String)> {
    // Top-level only: any leading whitespace means an indented use, not a
    // module authority definition. Reads and mutations through the owner
    // (`catalog.SCENARIOS =`, `SCENARIOS[x] =`, `==`, walrus) never match.
    if code.starts_with([' ', '\t']) || code.trim().is_empty() {
        return None;
    }
    let trimmed = code.trim_start();
    let mut name = String::new();
    for next in trimmed.chars() {
        if next.is_ascii_alphanumeric() || next == '_' {
            name.push(next);
        } else {
            break;
        }
    }
    if name.is_empty() || name.starts_with(|next: char| next.is_ascii_digit()) {
        return None;
    }
    let mut rest = trimmed[name.len()..].trim_start();
    if let Some(tail) = rest.strip_prefix(':') {
        let mut depth = 0usize;
        let mut end = tail.len();
        for (offset, byte) in tail.bytes().enumerate() {
            match byte {
                b'[' | b'(' | b'{' => depth += 1,
                b']' | b')' | b'}' => depth = depth.saturating_sub(1),
                b'=' if depth == 0 => {
                    end = offset;
                    break;
                }
                _ => {}
            }
        }
        if end == tail.len() {
            return None;
        }
        rest = tail[end..].trim_start();
    }
    let value = rest.strip_prefix('=')?;
    if value.starts_with('=') {
        return None;
    }
    let value = value.trim_start();
    if value.starts_with('=') {
        return None;
    }
    Some((name, value.to_owned()))
}

fn scan_generic_modules(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if matches!(trimmed, "mod utils;" | "mod common;" | "mod shared;")
            || matches!(
                trimmed,
                "pub mod utils;" | "pub mod common;" | "pub mod shared;"
            )
        {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "generic utils/common/shared module; use a domain module".into(),
            });
        }
    }
}

fn scan_api_leaks(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    let is_extension_api = PLUGIN_API_CRATES
        .iter()
        .any(|crate_name| path_has_component(path, crate_name));
    if !is_extension_api {
        return;
    }

    scan_api_evolution_guards(path, lines, findings);

    let aliases = forbidden_api_aliases(lines);
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let publicish = trimmed.starts_with("pub ") || trimmed.starts_with("pub(");
        if !publicish {
            continue;
        }

        let mut public_item = String::new();
        for item_line in &lines[index..] {
            public_item.push_str(item_line);
            public_item.push('\n');
            if public_item_end(item_line) {
                break;
            }
        }

        if contains_forbidden_api_surface(&public_item, &aliases) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API exposes internal runtime or transport type".into(),
            });
        }
    }
}

fn scan_api_evolution_guards(path: &Path, lines: &[&str], findings: &mut Vec<Finding>) {
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pub enum ") && !has_non_exhaustive_attr(lines, index) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API public enum missing #[non_exhaustive]".into(),
            });
        }
        if public_struct_exposes_fields(lines, index) && !has_non_exhaustive_attr(lines, index) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: "plugin/script API public field struct missing #[non_exhaustive]".into(),
            });
        }
    }
}

fn has_non_exhaustive_attr(lines: &[&str], index: usize) -> bool {
    let mut cursor = index;
    while cursor > 0 {
        cursor -= 1;
        let trimmed = lines[cursor].trim();
        if trimmed.is_empty() || trimmed.starts_with("///") || trimmed.starts_with("//!") {
            continue;
        }
        if trimmed.starts_with("#[") {
            if trimmed == "#[non_exhaustive]" {
                return true;
            }
            continue;
        }
        break;
    }
    false
}

fn public_struct_exposes_fields(lines: &[&str], index: usize) -> bool {
    let trimmed = lines[index].trim_start();
    if !trimmed.starts_with("pub struct ") {
        return false;
    }

    if let Some((_, tuple_tail)) = trimmed.split_once('(') {
        if tuple_tail.trim_start().starts_with("pub ") {
            return true;
        }
        if tuple_tail.contains(");") || tuple_tail.trim_end().ends_with(';') {
            return false;
        }
        for line in lines.iter().skip(index + 1) {
            let trimmed = line.trim_start();
            if trimmed.starts_with("pub ") || trimmed.starts_with("pub(") {
                return true;
            }
            if trimmed.contains(");") || trimmed.ends_with(';') {
                break;
            }
        }
        return false;
    }

    let mut saw_brace = false;
    for (offset, line) in lines[index..].iter().enumerate() {
        let trimmed = line.trim_start();
        if trimmed.contains('{') {
            saw_brace = true;
            if trimmed.contains("{ pub ") || trimmed.contains("{pub ") {
                return true;
            }
        }
        if saw_brace && offset > 0 && (trimmed.starts_with("pub ") || trimmed.starts_with("pub(")) {
            return true;
        }
        if saw_brace && trimmed.contains('}') {
            break;
        }
        if !saw_brace && trimmed.ends_with(';') {
            break;
        }
    }
    false
}

fn scan_api_manifests(root: &Path, findings: &mut Vec<Finding>) {
    for crate_name in PLUGIN_API_CRATES {
        let manifest = root.join("crates").join(crate_name).join("Cargo.toml");
        let Ok(source) = fs::read_to_string(&manifest) else {
            continue;
        };
        scan_api_manifest(&manifest, &source, findings);
    }
}

fn scan_api_manifest(path: &Path, source: &str, findings: &mut Vec<Finding>) {
    if !PLUGIN_API_CRATES
        .iter()
        .any(|crate_name| path_has_component(path, crate_name))
    {
        return;
    }

    for (index, line) in source.lines().enumerate() {
        let Some(dependency) = manifest_dependency_name(line.trim()) else {
            continue;
        };
        if FORBIDDEN_API_DEPENDENCIES.contains(&dependency) {
            findings.push(Finding {
                path: path.to_path_buf(),
                line: index + 1,
                message: format!("plugin/script API depends on internal crate `{dependency}`"),
            });
        }
    }
}

fn manifest_dependency_name(line: &str) -> Option<&str> {
    if let Some(section) = line
        .strip_prefix("[dependencies.")
        .or_else(|| line.strip_prefix("[dev-dependencies."))
        .or_else(|| line.strip_prefix("[build-dependencies."))
    {
        return section.strip_suffix(']');
    }

    if line.starts_with('[') {
        return None;
    }

    let (name, _) = line.split_once('=')?;
    let name = name.trim().trim_matches('"');
    (!name.is_empty()).then_some(name)
}

fn forbidden_api_aliases(lines: &[&str]) -> BTreeSet<String> {
    let mut aliases = BTreeSet::new();
    for line in lines {
        let trimmed = line.trim();
        if !(trimmed.starts_with("use ") || trimmed.starts_with("pub use ")) {
            continue;
        }
        if !contains_forbidden_api_surface(trimmed, &BTreeSet::new()) {
            continue;
        }
        if let Some((_, alias)) = trimmed.rsplit_once(" as ") {
            let alias = alias.trim_end_matches(';').trim();
            if !alias.is_empty() {
                aliases.insert(alias.to_owned());
            }
        }
    }
    aliases
}

fn contains_forbidden_api_surface(source: &str, aliases: &BTreeSet<String>) -> bool {
    FORBIDDEN_API_TYPES
        .iter()
        .chain(FORBIDDEN_API_TRANSPORTS.iter())
        .any(|token| source.contains(token))
        || aliases.iter().any(|alias| source.contains(alias))
}

fn public_item_end(line: &str) -> bool {
    let trimmed = line.trim_end();
    trimmed.ends_with(';')
        || trimmed.ends_with('{')
        || trimmed.ends_with('}')
        || trimmed.ends_with(',')
}

fn path_has_component(path: &Path, needle: &str) -> bool {
    path.components()
        .any(|component| component.as_os_str().to_string_lossy() == needle)
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}
