// Public-protocol fixture probes. One bounded mirror per process, never one per client.
// Images are received-state evidence; packet writes are deliberately not completion.
include!("player_scale_bots_receive.rs");
include!("player_scale_bots_ledger.rs");
include!("player_scale_bots_replies.rs");
include!("player_scale_bots_damage.rs");
include!("player_scale_bots_visibility.rs");
const MECHANIC_CELL_CAP: usize = 65536;
const MECHANIC_ENTITY_CAP: usize = 16384;
const MECHANIC_ACTION_CAP: usize = 65536;
const MECHANIC_SLOT_CAP: usize = 128;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MechanicSpec {
    schema_version: u32,
    #[serde(default)]
    entities: Vec<EntityProbe>,
    #[serde(default)]
    blocks: Vec<CellProbe>,
    #[serde(default)]
    containers: Vec<ContainerProbe>,
    #[serde(default)]
    actions: Vec<MechanicAction>,
    #[serde(default = "default_action_hz")]
    action_hz: u32,
    #[serde(default = "default_burst_action_hz")]
    burst_action_hz: u32,
    #[serde(default)]
    snapshot_only: bool,
}
fn default_action_hz() -> u32 {
    2
}
fn default_burst_action_hz() -> u32 {
    4
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EntityProbe {
    kind: String,
    count: u32,
    #[serde(default)]
    reader_images: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CellProbe {
    name: String,
    position: [i32; 3],
    #[serde(default)]
    reader_images: bool,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ContainerProbe {
    name: String,
    position: [i32; 3],
    #[serde(default)]
    bot: u32,
    #[serde(default)]
    slots: Option<usize>,
}
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct MechanicAction {
    bot: u32,
    kind: String,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    position: Option<[i32; 3]>,
    #[serde(default)]
    entity_kind: Option<String>,
    #[serde(default)]
    entity_uuid: Option<Uuid>,
    #[serde(default)]
    slot: Option<i16>,
    #[serde(default)]
    slot_item: Option<String>,
    #[serde(default)]
    button: i8,
    #[serde(default)]
    mode: u8,
    /// Contention may legitimately reject a quick-move. Keep its commit
    /// unclaimed and let the simultaneous native custody oracle decide.
    #[serde(default)]
    allow_no_change: bool,
    #[serde(default)]
    direction: Option<u32>,
    #[serde(default)]
    yaw: f32,
    #[serde(default)]
    pitch: f32,
    #[serde(default)]
    command: Option<String>,
    #[serde(default)]
    expected_feedback: Option<String>,
    #[serde(default)]
    window: Option<String>,
    #[serde(default)]
    at_ms: Option<u64>,
    #[serde(default)]
    bytes_per_second: Option<u64>,
}
#[derive(Clone, Serialize)]
struct CellImage {
    state: i32,
    name: String,
    properties: std::collections::BTreeMap<String, String>,
}
#[derive(Default, Serialize)]
struct CellReceipt {
    observer_bot: Option<u32>,
    initial: Option<i32>,
    final_state: Option<i32>,
    update_count: u64,
    fresh_reads: u64,
    received_by_window: std::collections::BTreeMap<String, u64>,
    #[serde(skip)]
    reader_states: Vec<Option<i32>>,
}
#[derive(Clone, PartialEq, Eq, Serialize)]
struct SlotImage {
    slot: usize,
    item_id: Option<u32>,
    name: Option<String>,
    count: i32,
    plain: bool,
}
#[derive(Default, Serialize)]
struct MenuReceipt {
    initial: Option<Vec<SlotImage>>,
    final_slots: Option<Vec<SlotImage>>,
    initial_player_inventory: Option<Vec<SlotImage>>,
    final_player_inventory: Option<Vec<SlotImage>>,
    carried: Option<SlotImage>,
    versions: Vec<i32>,
    versions_truncated: u64,
    updates: u64,
    container_slot_updates: u64,
    content_packets: u64,
    properties: std::collections::BTreeMap<i16, i16>,
    received_by_window: std::collections::BTreeMap<String, u64>,
}
impl MechanicAction {
    fn menu_slot(&self, menu: &MenuReceipt) -> Option<i16> {
        if let Some(slot) = self.slot {
            return Some(slot);
        }
        let item = self.slot_item.as_deref()?;
        let stack = menu
            .final_slots
            .as_deref()
            .unwrap_or_default()
            .iter()
            .chain(menu.final_player_inventory.as_deref().unwrap_or_default())
            .find(|stack| stack.count > 0 && stack.name.as_deref() == Some(item))?;
        i16::try_from(stack.slot).ok()
    }
}
#[derive(Serialize)]
struct EntityReceipt {
    uuid: Uuid,
    entity_id: i32,
    observer_bot: u32,
    kind: String,
    spawn_data: i32,
    initial_position: [f64; 3],
    final_position: [f64; 3],
    active: bool,
    visible_observers: u32,
    #[serde(skip)]
    visibility: Vec<u64>,
    #[serde(skip)]
    reader_positions: Vec<Option<[f64; 3]>>,
    move_count: u64,
    remove_count: u64,
    spawn_count: u64,
    metadata_updates: u64,
    death_events: u64,
    dead: bool,
    health: Option<f32>,
    flags: Option<i8>,
    pose: Option<String>,
    item: Option<SlotImage>,
    metadata: std::collections::BTreeMap<u8, Value>,
    received_by_window: std::collections::BTreeMap<String, u64>,
}
const MECHANIC_CENTER_TRACE_CAP: usize = 64;
#[derive(Serialize)]
struct MechanicCenterTrace {
    window: String,
    previous: Option<[i32; 2]>,
    center: [i32; 2],
}
#[derive(Default, Serialize)]
struct ReaderProgress {
    frames: u64,
    decoded_bytes: u64,
    position_updates: u64,
}
#[derive(Default, Serialize)]
struct PlayerReceipt {
    initial_health: Option<f32>,
    final_health: Option<f32>,
    initial_food: Option<i32>,
    final_food: Option<i32>,
    initial_saturation: Option<f32>,
    final_saturation: Option<f32>,
    food_credit_events: u64,
    food_credited: u64,
    saturation_credit_events: u64,
    saturation_credited: f64,
    damage_events: u64,
    damage_amount: f64,
    deaths: u64,
    respawns: u64,
    world_epoch: u64,
    initial_inventory: Option<Vec<SlotImage>>,
    final_inventory: Option<Vec<SlotImage>>,
    inventory_updates: u64,
    game_mode: Option<i32>,
    survival_mode_receipts: u64,
    survival_transition_admitted: u64,
    survival_transition_writes: u64,
    menu_opens: u64,
    initial_chunk_center: Option<[i32; 2]>,
    final_chunk_center: Option<[i32; 2]>,
    chunk_center_packets: u64,
    chunk_center_transitions: u64,
    chunk_center_trace: Vec<MechanicCenterTrace>,
    chunk_center_traces_truncated: u64,
    received_by_window: std::collections::BTreeMap<String, u64>,
    reader_progress_by_window: std::collections::BTreeMap<String, ReaderProgress>,
    reader_bytes_per_second: Option<u64>,
}
#[derive(Serialize)]
struct MechanicActionReceipt {
    action: usize,
    id: Option<String>,
    bot: u32,
    kind: String,
    window: String,
    at_ms: u64,
    admitted: u64,
    writes_completed: u64,
    completed: u64,
    awaited_menu_content: bool,
    dropped_late: u64,
    dropped_unavailable: u64,
    dropped_capacity: u64,
    errors: Vec<String>,
    completed_in_window: Option<String>,
    #[serde(skip)]
    consumed: bool,
    #[serde(skip)]
    window_index: usize,
}
struct Mechanics {
    spec: MechanicSpec,
    survival_on_run: bool,
    measurement_epoch: Option<Instant>,
    measurement_duration: Duration,
    measurement_closed: bool,
    registry: Vec<Option<CellImage>>,
    items: HashMap<u32, String>,
    item_limits: HashMap<u32, i32>,
    kinds: HashMap<i32, String>,
    cells: Vec<CellReceipt>,
    chunks: HashMap<(i32, i32), Vec<usize>>,
    positions: HashMap<i64, usize>,
    container_positions: HashSet<i64>,
    command_observers: HashSet<u32>,
    menus: Vec<MenuReceipt>,
    players: Vec<PlayerReceipt>,
    entities: HashMap<Uuid, EntityReceipt>,
    actions: Vec<MechanicActionReceipt>,
    wire_ids: HashMap<i32, Uuid>,
    damage_events: Vec<MechanicDamageTrace>,
    damage_events_received: u64,
    damage_events_unresolved: u64,
    damage_events_truncated: u64,
    damage_events_filtered: u64,
    projectile_hits: Vec<MechanicProjectileHit>,
    projectile_hit_keys: HashSet<(Uuid, u32)>,
    projectile_duplicate_hits: u64,
    identity_errors: u64,
    position_errors: u64,
    position_baseline_misses: u64,
    duplicate_spawns: u64,
    duplicate_removals: u64,
    ghost_updates: u64,
    capacity_errors: u64,
    explosions: Vec<Value>,
    command_replies: Vec<MechanicReply>,
    command_replies_received: u64,
    command_replies_truncated: u64,
    command_replies_unrenderable: u64,
    command_replies_filtered: u64,
    save_completions: u64,
    receive_windows: std::collections::BTreeMap<String, std::collections::BTreeMap<String, u64>>,
}
impl Mechanics {
    fn load(path: &std::path::Path, args: &Args) -> Result<Self> {
        let spec: MechanicSpec = serde_json::from_reader(std::fs::File::open(path)?)?;
        Self::new(spec, args)
    }
    fn new(spec: MechanicSpec, args: &Args) -> Result<Self> {
        ensure!(
            spec.schema_version == 1,
            "mechanic schema_version must be 1"
        );
        ensure!(
            spec.blocks.len() <= MECHANIC_CELL_CAP
                && spec.containers.len() <= 1024
                && spec.actions.len() <= 4096,
            "mechanic spec exceeds bounded probe capacity"
        );
        ensure!(
            spec.action_hz <= 1000 && spec.burst_action_hz <= 1000,
            "mechanic rate exceeds 1000 Hz"
        );
        let mut names = HashSet::new();
        let mut positions = HashMap::new();
        let mut chunks: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
        for (index, probe) in spec.blocks.iter().enumerate() {
            ensure!(
                !probe.name.is_empty() && names.insert(&probe.name),
                "duplicate/empty block probe name"
            );
            let [x, y, z] = probe.position;
            ensure!(
                (-64..320).contains(&y)
                    && x.unsigned_abs() < 30_000_000
                    && z.unsigned_abs() < 30_000_000,
                "probe outside overworld"
            );
            ensure!(
                positions.insert(pack_block_pos(x, y, z), index).is_none(),
                "duplicate physical cell probe"
            );
            chunks
                .entry((x.div_euclid(16), z.div_euclid(16)))
                .or_default()
                .push(index);
        }
        names.clear();
        for probe in &spec.containers {
            ensure!(
                names.insert(&probe.name)
                    && probe.bot < args.clients
                    && probe.slots.is_none_or(|n| n <= 54),
                "invalid container probe"
            );
        }
        let mut kinds = HashMap::new();
        let mut expected_count = 0usize;
        for probe in &spec.entities {
            let kind = mc_data::entity_types::entity_type_contract_26_1_2_by_name(&probe.kind)
                .context("unknown entity probe kind")?;
            ensure!(
                kinds
                    .insert(i32::try_from(kind.protocol_id)?, probe.kind.clone())
                    .is_none(),
                "duplicate entity probe kind"
            );
            expected_count = expected_count
                .checked_add(probe.count as usize)
                .context("entity probe count overflow")?;
        }
        ensure!(
            expected_count <= MECHANIC_ENTITY_CAP,
            "entity spec exceeds bounded identity capacity"
        );
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla/reports");
        let blocks = mc_data::blocks::load_blocks_report(root.join("blocks.json"))?;
        let max = blocks
            .iter()
            .flat_map(|b| b.states.iter().map(|s| s.id))
            .max()
            .context("empty block registry")? as usize;
        let mut registry = vec![None; max + 1];
        for block in blocks {
            for state in block.states {
                registry[state.id as usize] = Some(CellImage {
                    state: state.id as i32,
                    name: block.id.to_string(),
                    properties: state.properties,
                });
            }
        }
        let item_facts = mc_data::item_components::solaris_required_item_facts();
        let mut items = HashMap::new();
        let mut item_limits = HashMap::new();
        for item in mc_data::items::load_items_report(root.join("registries.json"))? {
            if let Some(limit) = item_facts
                .get(&item.id)
                .and_then(|facts| facts.max_stack_size)
            {
                item_limits.insert(item.protocol_id, i32::try_from(limit)?);
            }
            items.insert(item.protocol_id, item.id.to_string());
        }
        let mut actions = Vec::new();
        for (action_index, action) in spec.actions.iter().enumerate() {
            ensure!(
                action.bot < args.clients,
                "mechanic action bot outside client population"
            );
            ensure!(
                matches!(
                    action.kind.as_str(),
                    "open_container"
                        | "close_container"
                        | "click_slot"
                        | "use_block"
                        | "break_block"
                        | "attack_entity"
                        | "entity_interact"
                        | "use_item"
                        | "respawn"
                        | "set_direction"
                        | "set_rotation"
                        | "select_slot"
                        | "drop_stack"
                        | "command"
                        | "reader_rate_limit"
                        | "restore_reader"
                ),
                "unknown mechanic action kind {}",
                action.kind
            );
            if action.kind == "reader_rate_limit" {
                ensure!(
                    action.bytes_per_second.is_some_and(|rate| rate > 0),
                    "reader_rate_limit needs positive bytes_per_second"
                );
            } else {
                ensure!(
                    action.bytes_per_second.is_none(),
                    "bytes_per_second belongs only to reader_rate_limit"
                );
            }
            if matches!(
                action.kind.as_str(),
                "open_container" | "use_block" | "break_block"
            ) {
                ensure!(action.position.is_some(), "block action needs position");
            }
            if action.kind == "click_slot" {
                ensure!(
                    action.slot.is_some() != action.slot_item.is_some(),
                    "click_slot needs exactly one slot or slot_item"
                );
            } else {
                ensure!(
                    action.slot_item.is_none(),
                    "slot_item belongs only to click_slot"
                );
            }
            ensure!(
                action.mode <= 1 && (0..=1).contains(&action.button),
                "only pickup/quick-move and left/right buttons are supported"
            );
            ensure!(
                !action.allow_no_change || (action.kind == "click_slot" && action.mode == 1),
                "allow_no_change belongs only to contested quick-moves"
            );
            ensure!(
                action.direction.is_none_or(|d| d < 4)
                    && action.yaw.is_finite()
                    && action.pitch.is_finite(),
                "invalid action direction"
            );
            if action.kind == "set_direction" {
                ensure!(action.direction.is_some(), "set_direction needs direction");
            }
            if action.kind == "select_slot" {
                ensure!(
                    action.slot.is_some_and(|slot| (0..9).contains(&slot)),
                    "selected slot must be hotbar 0..8"
                );
            }
            if action.kind == "command" {
                ensure!(
                    action.command.as_ref().is_some_and(|s| !s.is_empty()
                        && s.len() <= 4096
                        && !s.contains(['\n', '\r'])),
                    "command action needs one bounded public command"
                );
            }
            let mut start_ms = 0u64;
            let mut matched = false;
            for (wi, window) in args.schedule.windows.iter().enumerate() {
                if window.recovery && action.at_ms.is_none() && action.window.is_none() {
                    start_ms += window.duration_ms;
                    continue;
                }
                let eligible = action
                    .window
                    .as_ref()
                    .is_none_or(|name| name == &window.name);
                let rate = if window.name == "burst" {
                    spec.burst_action_hz
                } else {
                    spec.action_hz
                };
                let ordinal = spec.actions[..=action_index]
                    .iter()
                    .filter(|a| {
                        a.bot == action.bot
                            && a.window.as_ref().is_none_or(|name| name == &window.name)
                            && a.at_ms.is_none()
                    })
                    .count() as u64;
                let at = if let Some(at) = action.at_ms {
                    if action.window.is_some() {
                        start_ms.checked_add(at).context("action offset overflow")?
                    } else {
                        at
                    }
                } else if rate > 0 {
                    start_ms + (ordinal * 1000).div_ceil(u64::from(rate))
                } else {
                    start_ms + window.duration_ms
                };
                let in_window = at >= start_ms && at < start_ms + window.duration_ms;
                if eligible && in_window {
                    ensure!(
                        !window.recovery || action.kind == "restore_reader",
                        "only restore_reader mechanic control is permitted in recovery"
                    );
                    matched = true;
                    actions.push(MechanicActionReceipt {
                        action: action_index,
                        bot: action.bot,
                        kind: action.kind.clone(),
                        window: window.name.clone(),
                        at_ms: at,
                        id: action.id.clone(),
                        admitted: 0,
                        writes_completed: 0,
                        completed: 0,
                        awaited_menu_content: false,
                        dropped_late: 0,
                        dropped_unavailable: 0,
                        dropped_capacity: 0,
                        errors: Vec::new(),
                        completed_in_window: None,
                        consumed: false,
                        window_index: wi,
                    });
                }
                start_ms += window.duration_ms;
            }
            ensure!(
                matched || spec.snapshot_only || args.mechanic_read_only,
                "action has no slot in active schedule"
            );
        }
        ensure!(
            actions.len() <= MECHANIC_ACTION_CAP,
            "mechanic scheduled action cap exceeded"
        );
        actions.sort_by_key(|a| (a.at_ms, a.bot, a.action));
        let container_positions = spec
            .containers
            .iter()
            .map(|probe| pack_block_pos(probe.position[0], probe.position[1], probe.position[2]))
            .collect();
        let mut command_observers: HashSet<_> = spec
            .actions
            .iter()
            .filter(|action| action.kind == "command")
            .map(|action| action.bot)
            .collect();
        command_observers.insert(0);
        Ok(Self {
            cells: spec
                .blocks
                .iter()
                .map(|probe| CellReceipt {
                    reader_states: if probe.reader_images {
                        vec![None; args.clients as usize]
                    } else {
                        Vec::new()
                    },
                    ..Default::default()
                })
                .collect(),
            menus: (0..spec.containers.len())
                .map(|_| MenuReceipt::default())
                .collect(),
            players: (0..args.clients)
                .map(|_| PlayerReceipt::default())
                .collect(),
            spec,
            survival_on_run: args.survival_on_run,
            measurement_epoch: None,
            measurement_duration: args.schedule.duration(),
            measurement_closed: false,
            registry,
            items,
            item_limits,
            kinds,
            chunks,
            positions,
            container_positions,
            command_observers,
            entities: HashMap::new(),
            actions,
            identity_errors: 0,
            position_errors: 0,
            position_baseline_misses: 0,
            duplicate_spawns: 0,
            duplicate_removals: 0,
            ghost_updates: 0,
            capacity_errors: 0,
            receive_windows: args
                .schedule
                .windows
                .iter()
                .map(|window| (window.name.clone(), Default::default()))
                .collect(),
            explosions: Vec::new(),
            command_replies: Vec::new(),
            command_replies_received: 0,
            command_replies_truncated: 0,
            command_replies_unrenderable: 0,
            command_replies_filtered: 0,
            save_completions: 0,
            wire_ids: HashMap::new(),
            damage_events: Vec::new(),
            damage_events_received: 0,
            damage_events_unresolved: 0,
            damage_events_truncated: 0,
            damage_events_filtered: 0,
            projectile_hits: Vec::new(),
            projectile_hit_keys: HashSet::new(),
            projectile_duplicate_hits: 0,
        })
    }
    fn begin_measurement(&mut self) -> Result<Instant> {
        ensure!(
            self.measurement_epoch.is_none(),
            "mechanic Run baseline already captured"
        );
        for cell in &mut self.cells {
            cell.initial = cell.final_state;
            cell.update_count = 0;
            cell.received_by_window.clear();
        }
        for menu in &mut self.menus {
            menu.initial.clone_from(&menu.final_slots);
            menu.initial_player_inventory
                .clone_from(&menu.final_player_inventory);
            menu.updates = 0;
            menu.container_slot_updates = 0;
            menu.received_by_window.clear();
        }
        for entity in self.entities.values_mut() {
            entity.initial_position = entity.final_position;
            entity.move_count = 0;
            entity.remove_count = 0;
            entity.spawn_count = 0;
            entity.metadata_updates = 0;
            entity.death_events = 0;
            entity.received_by_window.clear();
        }
        for player in &mut self.players {
            player.initial_health = player.final_health;
            player.initial_food = player.final_food;
            player.initial_saturation = player.final_saturation;
            player.food_credit_events = 0;
            player.food_credited = 0;
            player.saturation_credit_events = 0;
            player.saturation_credited = 0.0;
            player.initial_inventory.clone_from(&player.final_inventory);
            player.damage_events = 0;
            player.damage_amount = 0.0;
            player.deaths = 0;
            player.respawns = 0;
            player.inventory_updates = 0;
            player.survival_mode_receipts = 0;
            player.menu_opens = 0;
            player.initial_chunk_center = player.final_chunk_center;
            player.chunk_center_packets = 0;
            player.chunk_center_transitions = 0;
            player.chunk_center_trace.clear();
            player.chunk_center_traces_truncated = 0;
            player.received_by_window.clear();
            player.reader_progress_by_window.clear();
        }
        for receipts in self.receive_windows.values_mut() {
            receipts.clear();
        }
        self.explosions.clear();
        self.damage_events.clear();
        self.damage_events_received = 0;
        self.damage_events_unresolved = 0;
        self.damage_events_truncated = 0;
        self.damage_events_filtered = 0;
        self.projectile_hits.clear();
        self.projectile_hit_keys.clear();
        self.projectile_duplicate_hits = 0;
        self.command_replies.clear();
        self.command_replies_received = 0;
        self.command_replies_truncated = 0;
        self.command_replies_unrenderable = 0;
        self.command_replies_filtered = 0;
        self.save_completions = 0;
        let instant = Instant::now();
        self.measurement_epoch = Some(instant);
        Ok(instant)
    }
    fn nutrition(&mut self, bot: u32, food: i32, saturation: f32, window: Option<&str>) {
        let player = &mut self.players[bot as usize];
        let food_credit = window.is_some() && player.final_food.is_some_and(|before| food > before);
        let saturation_credit = window.is_some()
            && player
                .final_saturation
                .is_some_and(|before| saturation > before);
        if food_credit {
            player.food_credit_events += 1;
            player.food_credited +=
                (i64::from(food) - i64::from(player.final_food.expect("prior food"))) as u64;
        }
        if saturation_credit {
            player.saturation_credit_events += 1;
            player.saturation_credited += f64::from(saturation)
                - f64::from(player.final_saturation.expect("prior saturation"));
        }
        if player.initial_food.is_none() || window.is_none() {
            player.initial_food = Some(food);
            player.initial_saturation = Some(saturation);
        }
        player.final_food = Some(food);
        player.final_saturation = Some(saturation);
        if food_credit {
            self.receipt(window, "food_credit");
        }
        if saturation_credit {
            self.receipt(window, "saturation_credit");
        }
    }
    fn chunk_center(&mut self, bot: u32, center: [i32; 2], window: Option<&str>) {
        if !self.accepts_receive(window) {
            return;
        }
        let player = &mut self.players[bot as usize];
        let previous = player.final_chunk_center;
        player.final_chunk_center = Some(center);
        let Some(window) = window else {
            player.initial_chunk_center = Some(center);
            return;
        };
        player.chunk_center_packets += 1;
        if previous == Some(center) {
            return;
        }
        if previous.is_some() {
            player.chunk_center_transitions += 1;
        }
        if player.chunk_center_trace.len() < MECHANIC_CENTER_TRACE_CAP {
            player.chunk_center_trace.push(MechanicCenterTrace {
                window: window.to_owned(),
                previous,
                center,
            });
        } else {
            player.chunk_center_traces_truncated += 1;
        }
        if previous.is_some() {
            self.receipt(Some(window), "chunk_center_change");
        }
    }
    fn accepts_receive(&self, _window: Option<&str>) -> bool {
        !self.measurement_closed
            && self
                .measurement_epoch
                .is_none_or(|epoch| epoch.elapsed() < self.measurement_duration)
    }
    fn receipt(&mut self, window: Option<&str>, kind: &str) {
        if let Some(window) = window {
            if !self.receive_windows.contains_key(window) {
                self.receive_windows
                    .insert(window.to_owned(), Default::default());
            }
            increment_mechanic_receipt(
                self.receive_windows
                    .get_mut(window)
                    .expect("receive window"),
                kind,
            );
        }
    }
    fn cell(&mut self, bot: u32, index: usize, state: i32, fresh: bool, window: Option<&str>) {
        if !self.accepts_receive(window) {
            return;
        }
        if self
            .registry
            .get(state as usize)
            .and_then(Option::as_ref)
            .is_none()
        {
            self.position_errors += 1;
            return;
        }
        let cell = &mut self.cells[index];
        if let Some(reader) = cell.reader_states.get_mut(bot as usize) {
            *reader = Some(state);
        }
        if cell.observer_bot.is_none() && fresh {
            cell.observer_bot = Some(bot);
            cell.initial = Some(state);
        }
        if cell.observer_bot.is_none() || (fresh && cell.observer_bot != Some(bot)) {
            return;
        }
        // Block deltas carry absolute states, unlike relative entity moves.
        // A current viewer can take over; repeated equal states do not add work.
        // Keep chunk-image admission single-observer so delayed restreams cannot
        // overwrite another viewer's newer delta.
        if !fresh {
            cell.observer_bot = Some(bot);
        }
        let changed = cell.final_state.is_some_and(|before| before != state);
        if fresh {
            cell.fresh_reads += 1;
        }
        if changed && window.is_some() {
            cell.update_count += 1;
        }
        cell.final_state = Some(state);
        if window.is_none() {
            cell.initial = Some(state);
        }
        if let Some(window) = window {
            increment_mechanic_receipt(&mut cell.received_by_window, window);
        }
        if fresh {
            self.receipt(window, "cell_fresh");
        }
        if changed {
            self.receipt(window, "cell_update");
        }
    }
    fn update_cell(&mut self, bot: u32, position: i64, state: i32, window: Option<&str>) {
        if let Some(&index) = self.positions.get(&position) {
            self.cell(bot, index, state, false, window);
        }
    }
    fn slot(&self, slot: usize, stack: &ItemStack) -> SlotImage {
        if stack.count <= 0 {
            SlotImage {
                slot,
                item_id: None,
                name: None,
                count: 0,
                plain: true,
            }
        } else {
            SlotImage {
                slot,
                item_id: Some(stack.item_id),
                name: self.items.get(&stack.item_id).cloned(),
                count: stack.count,
                plain: *stack == ItemStack::new(stack.item_id, stack.count),
            }
        }
    }
    fn spawn(&mut self, bot: u32, packet: &AddEntity, window: Option<&str>) {
        if !self.accepts_receive(window) {
            return;
        }
        let Some(kind) = self.kinds.get(&packet.entity_type_id) else {
            return;
        };
        if ![packet.x, packet.y, packet.z]
            .iter()
            .all(|x| x.is_finite() && x.abs() < 30_000_000.0)
        {
            self.position_errors += 1;
            return;
        }
        if self.wire_ids.get(&packet.entity_id).is_some_and(|old| {
            *old != packet.uuid && self.entities.get(old).is_some_and(|entity| entity.active)
        }) {
            self.identity_errors += 1;
            return;
        }
        if let Some(entity) = self.entities.get_mut(&packet.uuid) {
            if entity.entity_id != packet.entity_id || entity.kind != *kind {
                self.identity_errors += 1;
                return;
            }
            let primary_visible = entity.visible_to(entity.observer_bot);
            if !entity.show(bot) {
                self.duplicate_spawns += 1;
            }
            if let Some(reader) = entity.reader_positions.get_mut(bot as usize) {
                *reader = Some([packet.x, packet.y, packet.z]);
            }
            if entity.observer_bot == bot || !primary_visible {
                entity.observer_bot = bot;
                entity.spawn_count += u64::from(window.is_some());
                entity.final_position = [packet.x, packet.y, packet.z];
                if window.is_none() {
                    entity.initial_position = entity.final_position;
                }
            }
        } else {
            if self.entities.len() >= MECHANIC_ENTITY_CAP {
                self.capacity_errors += 1;
                return;
            }
            self.entities.insert(
                packet.uuid,
                EntityReceipt {
                    uuid: packet.uuid,
                    entity_id: packet.entity_id,
                    observer_bot: bot,
                    kind: kind.clone(),
                    spawn_data: packet.data,
                    initial_position: [packet.x, packet.y, packet.z],
                    final_position: [packet.x, packet.y, packet.z],
                    active: true,
                    visible_observers: 1,
                    visibility: initial_entity_visibility(self.players.len(), bot),
                    reader_positions: if self
                        .spec
                        .entities
                        .iter()
                        .any(|probe| probe.reader_images && probe.kind == *kind)
                    {
                        let mut positions = vec![None; self.players.len()];
                        positions[bot as usize] = Some([packet.x, packet.y, packet.z]);
                        positions
                    } else {
                        Vec::new()
                    },
                    move_count: 0,
                    death_events: 0,
                    dead: false,
                    remove_count: 0,
                    spawn_count: u64::from(window.is_some()),
                    metadata_updates: 0,
                    health: None,
                    flags: None,
                    pose: None,
                    item: None,
                    metadata: Default::default(),
                    received_by_window: Default::default(),
                },
            );
        }
        self.wire_ids.insert(packet.entity_id, packet.uuid);
        self.receipt(window, "entity_spawn");
    }
    fn movement(
        &mut self,
        bot: u32,
        id: i32,
        position: [f64; 3],
        relative: bool,
        window: Option<&str>,
    ) {
        if !self.accepts_receive(window) {
            return;
        }
        let Some(uuid) = self.wire_ids.get(&id) else {
            return;
        };
        let entity = self.entities.get_mut(uuid).expect("indexed identity");
        if !entity.visible_to(bot) {
            self.ghost_updates += 1;
            return;
        }
        if let Some(reader) = entity.reader_positions.get_mut(bot as usize) {
            let next = if relative {
                reader.map(|before| std::array::from_fn(|i| before[i] + position[i]))
            } else {
                Some(position)
            };
            if next.is_some_and(|value| {
                value
                    .iter()
                    .all(|x| x.is_finite() && x.abs() < 30_000_000.0)
            }) {
                *reader = next;
            } else {
                self.position_errors += 1;
                return;
            }
        }
        if entity.observer_bot != bot {
            if entity.visible_to(entity.observer_bot) {
                return;
            }
            if relative {
                self.position_baseline_misses += 1;
                return;
            }
            entity.observer_bot = bot;
        }
        let next = if relative {
            std::array::from_fn(|i| entity.final_position[i] + position[i])
        } else {
            position
        };
        if !next.iter().all(|x| x.is_finite() && x.abs() < 30_000_000.0) {
            self.position_errors += 1;
            return;
        }
        let changed = next != entity.final_position;
        if changed {
            entity.move_count += u64::from(window.is_some());
            entity.final_position = next;
        }
        if window.is_none() {
            entity.initial_position = entity.final_position;
        }
        if let Some(window) = window {
            increment_mechanic_receipt(&mut entity.received_by_window, window);
        }
        if changed {
            self.receipt(window, "entity_move");
        }
    }
    fn remove(&mut self, bot: u32, id: i32, window: Option<&str>) {
        if !self.accepts_receive(window) {
            return;
        }
        if let Some(uuid) = self.wire_ids.get(&id) {
            let entity = self.entities.get_mut(uuid).expect("indexed identity");
            if entity.hide(bot) {
                entity.remove_count += u64::from(window.is_some());
                self.receipt(window, "entity_remove");
            } else {
                // Respawn clears a view implicitly; subsequent remove packets
                // are idempotent cleanup, not motion/metadata on a ghost.
                self.duplicate_removals += 1;
            }
        }
    }
    fn json(&self, elapsed: Option<Duration>, read_only: bool) -> Value {
        let blocks:serde_json::Map<String,Value>=self.spec.blocks.iter().zip(&self.cells).map(|(probe,cell)| {
            let image=|state:Option<i32>| state.and_then(|state|self.registry.get(state as usize).and_then(Option::as_ref));
            (probe.name.clone(),json!({"position":probe.position,"observer_bot":cell.observer_bot,"initial":image(cell.initial),"final":image(cell.final_state),"update_count":cell.update_count,"fresh_reads":cell.fresh_reads,"received_by_window":cell.received_by_window}))
        }).collect();
        let containers:serde_json::Map<String,Value>=self.spec.containers.iter().zip(&self.menus).map(|(probe,menu)|(probe.name.clone(),json!({"position":probe.position,"bot":probe.bot,"initial":menu.initial,"final":menu.final_slots,"initial_player_inventory":menu.initial_player_inventory,"final_player_inventory":menu.final_player_inventory,"carried":menu.carried,"versions":menu.versions,"versions_truncated":menu.versions_truncated,"updates":menu.updates,"container_slot_updates":menu.container_slot_updates,"content_packets":menu.content_packets,"properties":menu.properties,"received_by_window":menu.received_by_window}))).collect();
        let entities:serde_json::Map<String,Value>=self.spec.entities.iter().map(|probe| {
            let members:Vec<_>=self.entities.values().filter(|e| e.kind==probe.kind).collect();
            (probe.kind.clone(),json!({"expected":probe.count,"observed":members.len(),"active":members.iter().filter(|e|e.active).count(),"moved":members.iter().filter(|e|e.move_count>0).count(),"removed":members.iter().filter(|e|e.remove_count>0).count(),"samples":members}))
        }).collect();
        let elapsed_ms = elapsed.map(|e| e.as_millis());
        let actions: Vec<_> = self
            .actions
            .iter()
            .map(|a| {
                let mut value = serde_json::to_value(a).expect("action receipt");
                value["offered"] = json!(u64::from(
                    !read_only
                        && !self.spec.snapshot_only
                        && elapsed_ms.is_some_and(|t| t >= u128::from(a.at_ms))
                ));
                if value["offered"] == 1
                    && a.admitted == 0
                    && a.dropped_unavailable == 0
                    && a.dropped_capacity == 0
                {
                    value["dropped_late"] = json!(1);
                }
                value
            })
            .collect();
        let survival_offered = u64::from(
            self.survival_on_run && !read_only && !self.spec.snapshot_only && elapsed.is_some(),
        );
        let missing_cells: Vec<_> = self
            .spec
            .blocks
            .iter()
            .zip(&self.cells)
            .filter(|(_, c)| c.initial.is_none())
            .map(|(p, _)| &p.name)
            .collect();
        let missing_menus: Vec<_> = self
            .spec
            .containers
            .iter()
            .zip(&self.menus)
            .filter(|(_, m)| m.initial.is_none())
            .map(|(p, _)| &p.name)
            .collect();
        let windows: serde_json::Map<String, Value> = self.receive_windows.iter().map(|(name, receipts)| {
            let count = |key: &str| receipts.get(key).copied().unwrap_or(0);
            (name.clone(), json!({"entity_moves": count("entity_move"), "block_updates": count("cell_update"),
                "container_updates": count("container_update"), "metadata_changes": count("metadata_change"),
                "health_damage": count("health_damage"), "deaths": count("death"), "respawns": count("respawn"),
                "projectile_spawns": count("projectile_spawn"), "explosions": count("explosion"),
                "chunk_center_changes": count("chunk_center_change"),
                "food_credit_events": count("food_credit"), "saturation_credit_events": count("saturation_credit"),
                "save_completions": count("save_completion")}))
        }).collect();
        let players: Vec<_> = self
            .players
            .iter()
            .enumerate()
            .map(|(bot, player)| {
                let mut value = serde_json::to_value(player).expect("player receipt");
                let mut visible: Vec<_> = self
                    .entities
                    .values()
                    .filter(|entity| entity.visible_to(bot as u32))
                    .collect();
                visible.sort_unstable_by_key(|entity| entity.uuid);
                value["final_observed_entities"] = json!(visible.iter().map(|entity| {
                json!({"uuid":entity.uuid,"entity_id":entity.entity_id,"kind":entity.kind,
                    "final_position":entity.reader_positions.get(bot).copied().flatten()})
            }).collect::<Vec<_>>());
                let blocks: serde_json::Map<String, Value> = self
                    .spec
                    .blocks
                    .iter()
                    .zip(&self.cells)
                    .filter(|(probe, _)| probe.reader_images)
                    .map(|(probe, cell)| {
                        let image = cell
                            .reader_states
                            .get(bot)
                            .copied()
                            .flatten()
                            .and_then(|state| self.registry.get(state as usize))
                            .and_then(Option::as_ref);
                        (probe.name.clone(), json!(image))
                    })
                    .collect();
                value["final_blocks"] = Value::Object(blocks);
                value
            })
            .collect();
        let mut report = json!({
            "schema_version": 1, "read_only": read_only || self.spec.snapshot_only,
            "baseline_captured": self.measurement_epoch.is_some(),
            "baseline_scope": "received images rebased once before Run; pre-Run in-flight receives refine baseline without work; measured counters use actual receive windows, not source-generation ticks",
            "movement_scope": "actual received SetCenterChunk snapshot-derived cache centers; not per-movement acknowledgements or client-predicted walking distance",
            "nutrition_scope": "actual received food/saturation increases, excluding respawn resets; nutrition credits are not exclusive consumption attribution",
            "players": players,
            "reader_scope": "per-bot decoded frames/bytes in actual receive windows; selected readers limit pre-decoding server socket reads via bounded Tokio relay; final_observed_entities includes currently visible declared probes only, not authoritative world membership",
            "received_by_window": self.receive_windows, "explosions": self.explosions,
            "explosion_scope": "bounded ClientboundExplode rows from primary observer bot 0 only; per-window explosion receipt counts include all observers; rows are received effects, not authoritative source attribution",
            "damage_events": self.damage_events, "damage_events_received": self.damage_events_received,
            "damage_events_unresolved": self.damage_events_unresolved, "damage_events_truncated": self.damage_events_truncated,
            "projectile_hits": self.projectile_hits, "projectile_duplicate_hits": self.projectile_duplicate_hits,
            "damage_events_filtered": self.damage_events_filtered,
            "damage_scope": "actual DamageEvent references resolved to previously received UUIDs; retain own-victim or primary-view declared-victim traces once, filter other view fanout; unresolved IDs remain null",
            "deaths": self.players.iter().map(|player| player.deaths).sum::<u64>(),
            "respawns": self.players.iter().map(|player| player.respawns).sum::<u64>(),
            "command_replies": self.command_replies, "command_replies_received": self.command_replies_received,
            "command_replies_truncated": self.command_replies_truncated,
            "command_replies_unrenderable": self.command_replies_unrenderable,
            "command_replies_filtered": self.command_replies_filtered,
            "save_completions": self.save_completions,
            "command_reply_scope": "literal public SystemChat at command-observer bots in actual receive window; generated harness chat echoes filtered from retention; dynamic components not guessed",
            "missing_probes": {"blocks": missing_cells, "containers": missing_menus},
            "identity_errors": self.identity_errors, "position_errors": self.position_errors,
            "duplicate_spawns": self.duplicate_spawns, "ghost_updates": self.ghost_updates,
            "duplicate_removals": self.duplicate_removals,
            "capacity_errors": self.capacity_errors,
            "position_baseline_misses": self.position_baseline_misses,
            "entity_scope": "one body/position primary per UUID plus compact visibility bits for every client; active means at least one current received view; duplicate/ghost checks cover every view of known probed IDs; removals include visibility unloads, not authoritative world death",
            "cell_scope": "first fresh-section observer owns each physical cell; absent probes are null, never assumed air",
            "bounds": {"cells": MECHANIC_CELL_CAP, "entities": MECHANIC_ENTITY_CAP, "actions": MECHANIC_ACTION_CAP,
                "menu_slots": MECHANIC_SLOT_CAP, "reply_count": MECHANIC_REPLY_CAP, "reply_bytes": MECHANIC_REPLY_BYTES, "damage_events": MECHANIC_DAMAGE_CAP,
                "chunk_center_traces_per_bot": MECHANIC_CENTER_TRACE_CAP},
            "unavailable_evidence": ["these protocol probes do not expose server command queue age",
                "removal packets do not distinguish death from view unload",
                "villager home/job Brain memories are not transmitted by public entity metadata"]
        });
        report["blocks"] = Value::Object(blocks);
        report["containers"] = Value::Object(containers);
        report["entities"] = Value::Object(entities);
        report["actions"] = Value::Array(actions);
        report["windows"] = Value::Object(windows);
        for player in report["players"]
            .as_array_mut()
            .expect("serialized player array")
        {
            player["survival_transition_offered"] = json!(survival_offered);
            let admitted = player["survival_transition_admitted"]
                .as_u64()
                .expect("serialized admission");
            player["survival_transition_dropped_late"] =
                json!(survival_offered.saturating_sub(admitted));
            player["measured_chunk_center_proof"] = if player["chunk_center_packets"]
                .as_u64()
                .expect("serialized center packets")
                > 0
            {
                json!({"source":"SetCenterChunk","scope":"received cache-center packets; trace contains distinct center changes"})
            } else {
                Value::Null
            };
        }
        report
    }
}

#[derive(Clone)]
enum MechanicExpectation {
    Menu(usize, u64),
    MenuChanged(MenuTransfer),
    Cell(i64, i32, bool),
    Entity(Uuid, Option<f32>, u64, Option<u64>),
    InventoryDebit {
        slot: usize,
        item_id: u32,
        before: i32,
        all: bool,
    },
    Respawn(u64),
    Feedback(String),
}
struct PendingMechanic {
    receipt: usize,
    expectation: MechanicExpectation,
    sent: Instant,
}
struct PendingMenuClose {
    receipt: usize,
    menu: usize,
    content_before: u64,
    offered: Instant,
}
#[derive(Default)]
struct BotMechanics {
    assigned: Vec<usize>,
    cursor: usize,
    pending: VecDeque<PendingMechanic>,
    opening: Option<usize>,
    menu: Option<(i32, usize, i32)>,
    deferred_close: Option<PendingMenuClose>,
    selected_slot: usize,
    last_unavailable_reason: Option<&'static str>,
    last_command: Option<(usize, Instant)>,
    last_position_sync: Option<(Instant, [f64; 3])>,
    /// Actions held for a server event, in order (receipts).
    held: VecDeque<usize>,
    /// The head of `held` waits for the next received pose sync.
    held_pose_blocked: bool,
    /// This bot sent a teleport command after its last received pose sync.
    pose_sync_expected: bool,
    /// A pose sync arrived since the held head was last evaluated.
    pose_synced: bool,
}

impl Bot {
    fn complete_mechanics(&mut self, window: Option<&str>) {
        let Some(shared) = self.mechanics.as_ref() else {
            return;
        };
        let mut state = shared.lock();
        self.mechanic_bot.pending.retain(|pending| {
            let completed = match &pending.expectation {
                MechanicExpectation::Menu(i, before) => state.menus[*i].content_packets > *before,
                MechanicExpectation::MenuChanged(transfer) => {
                    transfer.completed(&state.menus[transfer.menu])
                }
                MechanicExpectation::Cell(pos, before, air) => {
                    state.positions.get(pos).is_some_and(|i| {
                        state.cells[*i].final_state.is_some_and(|after| {
                            after != *before && (!air || self.facts.get(after).passable)
                        })
                    })
                }
                MechanicExpectation::Entity(uuid, health, deaths, menu_opens) => menu_opens
                    .map_or_else(
                        || {
                            state.entities.get(uuid).is_some_and(|e| {
                                e.death_events > *deaths
                                    || e.health.zip(*health).is_some_and(|(a, b)| a < b)
                            })
                        },
                        |before| state.players[self.index as usize].menu_opens > before,
                    ),
                MechanicExpectation::InventoryDebit {
                    slot,
                    item_id,
                    before,
                    all,
                } => state.players[self.index as usize]
                    .final_inventory
                    .as_ref()
                    .and_then(|inventory| inventory.get(*slot))
                    .is_some_and(|after| {
                        if *all {
                            after.count == 0
                        } else {
                            after.item_id != Some(*item_id) || after.count < *before
                        }
                    }),
                MechanicExpectation::Respawn(before) => {
                    state.players[self.index as usize].respawns > *before
                }
                MechanicExpectation::Feedback(_) => state.actions[pending.receipt].completed > 0,
            };
            let receipt = &mut state.actions[pending.receipt];
            if completed {
                if receipt.completed == 0 {
                    receipt.completed = 1;
                    receipt.completed_in_window = window.map(str::to_owned);
                }
                false
            } else if pending.sent.elapsed() >= ACTION_TIMEOUT {
                receipt
                    .errors
                    .push("received-state completion timeout".into());
                false
            } else {
                true
            }
        });
    }
    async fn close_received_menu(&mut self, client: &mut Client) -> Result<()> {
        let Some(close) = self.mechanic_bot.deferred_close.as_ref() else {
            return Ok(());
        };
        let shared = self.mechanics.as_ref().expect("deferred menu close state");
        if close.offered.elapsed() >= ACTION_TIMEOUT {
            shared.lock().actions[close.receipt]
                .errors
                .push("menu content missing before deferred close deadline".into());
            self.mechanic_bot.deferred_close = None;
            return Ok(());
        }
        let Some((id, index, _)) = self.mechanic_bot.menu else {
            return Ok(());
        };
        if index != close.menu || shared.lock().menus[index].content_packets <= close.content_before
        {
            return Ok(());
        }
        // This is the one already-offered close, not a new/retried load slot.
        // Independent movement, command and mechanic offers keep their wall clock.
        let close = self
            .mechanic_bot
            .deferred_close
            .take()
            .expect("pending close");
        self.mechanic_bot.menu = None;
        client
            .write_packet(&ServerboundContainerClose { container_id: id })
            .await?;
        shared.lock().actions[close.receipt].writes_completed = 1;
        Ok(())
    }
    async fn mechanic_use_block(&mut self, client: &mut Client, position: [i32; 3]) -> Result<()> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .context("mechanic sequence overflow")?;
        client
            .write_packet(&ServerboundUseItemOn {
                hand: InteractionHand::MainHand,
                position: pack_block_pos(position[0], position[1], position[2]),
                direction: Direction::Up,
                cursor_x: 0.5,
                cursor_y: 1.0,
                cursor_z: 0.5,
                inside: false,
                world_border_hit: false,
                sequence: self.sequence,
            })
            .await?;
        Ok(())
    }
    async fn mechanic_action(&mut self, client: &mut Client, receipt: usize) -> Result<bool> {
        let shared = Arc::clone(self.mechanics.as_ref().expect("mechanic action shared"));
        let (kind, position, slot, button, mode, direction, yaw, pitch, command, expectation) = {
            let mut state = shared.lock();
            let action = &state.spec.actions[state.actions[receipt].action];
            if action.kind == "close_container"
                && self.mechanic_bot.deferred_close.is_none()
                && let Some((menu, before)) = self.mechanic_bot.pending.iter().find_map(|pending| {
                    match &pending.expectation {
                        MechanicExpectation::Menu(menu, before) => Some((*menu, *before)),
                        _ => None,
                    }
                })
            {
                self.mechanic_bot.deferred_close = Some(PendingMenuClose {
                    receipt,
                    menu,
                    content_before: before,
                    offered: Instant::now(),
                });
                state.actions[receipt].admitted = 1;
                state.actions[receipt].awaited_menu_content = true;
                return Ok(true);
            }
            if matches!(
                action.kind.as_str(),
                "open_container" | "close_container" | "entity_interact"
            ) && (self.mechanic_bot.deferred_close.is_some()
                || self.mechanic_bot.opening.is_some()
                || self.mechanic_bot.pending.iter().any(|pending| {
                    matches!(
                        &pending.expectation,
                        MechanicExpectation::Menu(..)
                            | MechanicExpectation::MenuChanged(_)
                            | MechanicExpectation::Entity(_, _, _, Some(_))
                    )
                }))
            {
                self.mechanic_bot.last_unavailable_reason = Some("menu_open_or_transfer_pending");
                return Ok(false);
            }
            let mut slot = action.slot;
            let expectation = match action.kind.as_str() {
                "open_container" => {
                    let index =
                        state.spec.containers.iter().position(|c| {
                            Some(c.position) == action.position && c.bot == self.index
                        });
                    let Some(index) = index else {
                        self.mechanic_bot.last_unavailable_reason =
                            Some("container_probe_not_owned");
                        return Ok(false);
                    };
                    Some(MechanicExpectation::Menu(
                        index,
                        state.menus[index].content_packets,
                    ))
                }
                "click_slot" => {
                    let Some((_, i, _)) = self.mechanic_bot.menu else {
                        return Ok(false);
                    };
                    if self.mechanic_bot.pending.iter().any(|pending| {
                        matches!(&pending.expectation,
                            MechanicExpectation::MenuChanged(transfer) if transfer.menu == i)
                    }) {
                        return Ok(false);
                    }
                    let Some(resolved) = action.menu_slot(&state.menus[i]) else {
                        return Ok(false);
                    };
                    slot = Some(resolved);
                    let slot = usize::try_from(resolved)?;
                    if state
                        .predicted_carried(i, slot, action.button, action.mode)
                        .is_none()
                    {
                        return Ok(false);
                    }
                    if action.allow_no_change {
                        None
                    } else {
                        let Some(transfer) =
                            MenuTransfer::capture(i, &state.menus[i], slot, action.mode == 1)
                        else {
                            return Ok(false);
                        };
                        Some(MechanicExpectation::MenuChanged(transfer))
                    }
                }
                "use_block" | "break_block" => {
                    let p = action.position.expect("validated position");
                    let pos = pack_block_pos(p[0], p[1], p[2]);
                    if self.mechanic_bot.pending.iter().any(|pending| {
                        matches!(&pending.expectation,
                            MechanicExpectation::Cell(other, _, _) if *other == pos)
                    }) {
                        self.mechanic_bot.last_unavailable_reason = Some("target_cell_pending");
                        return Ok(false);
                    }
                    let before = state
                        .positions
                        .get(&pos)
                        .and_then(|i| state.cells[*i].final_state);
                    let Some(before) = before else {
                        self.mechanic_bot.last_unavailable_reason =
                            Some("target_cell_not_received");
                        return Ok(false);
                    };
                    Some(MechanicExpectation::Cell(
                        pos,
                        before,
                        action.kind == "break_block",
                    ))
                }
                "attack_entity" | "entity_interact" => {
                    let entity = state
                        .entities
                        .values()
                        .filter(|e| {
                            e.active
                                && e.visible_to(self.index)
                                && action
                                    .entity_kind
                                    .as_ref()
                                    .is_none_or(|kind| &e.kind == kind)
                                && action.entity_uuid.is_none_or(|uuid| e.uuid == uuid)
                        })
                        .filter(|e| {
                            e.final_position
                                .iter()
                                .zip([self.x, self.y + 1.62, self.z])
                                .map(|(a, b)| (a - b).powi(2))
                                .sum::<f64>()
                                <= 36.0
                        })
                        .min_by_key(|e| e.entity_id);
                    let Some(entity) = entity else {
                        return Ok(false);
                    };
                    if self.mechanic_bot.pending.iter().any(|pending| {
                        matches!(&pending.expectation,
                            MechanicExpectation::Entity(uuid, _, _, _) if *uuid == entity.uuid)
                    }) {
                        return Ok(false);
                    }
                    Some(MechanicExpectation::Entity(
                        entity.uuid,
                        entity.health,
                        entity.death_events,
                        (action.kind == "entity_interact")
                            .then_some(state.players[self.index as usize].menu_opens),
                    ))
                }
                "drop_stack" | "use_item" => {
                    let slot = 36 + self.mechanic_bot.selected_slot;
                    if self.mechanic_bot.pending.iter().any(|pending| {
                        matches!(&pending.expectation,
                            MechanicExpectation::InventoryDebit {slot: other, ..} if *other == slot)
                    }) {
                        return Ok(false);
                    }
                    let Some(before) = state.players[self.index as usize]
                        .final_inventory
                        .as_ref()
                        .and_then(|inventory| inventory.get(slot))
                    else {
                        return Ok(false);
                    };
                    let Some(item_id) = before.item_id else {
                        return Ok(false);
                    };
                    Some(MechanicExpectation::InventoryDebit {
                        slot,
                        item_id,
                        before: before.count,
                        all: action.kind == "drop_stack",
                    })
                }
                "respawn" => Some(MechanicExpectation::Respawn(
                    state.players[self.index as usize].respawns,
                )),
                "command" => action
                    .expected_feedback
                    .clone()
                    .map(MechanicExpectation::Feedback),
                _ => None,
            };
            (
                action.kind.clone(),
                action.position,
                slot,
                action.button,
                action.mode,
                action.direction,
                action.yaw,
                action.pitch,
                action.command.clone(),
                expectation,
            )
        };
        if self.mechanic_bot.pending.len() >= ACTION_CAP {
            return Ok(false);
        }
        if matches!(
            kind.as_str(),
            "open_container" | "use_block" | "break_block"
        ) {
            let p = position.expect("validated position");
            if p.iter()
                .map(|x| f64::from(*x) + 0.5)
                .zip([self.x, self.y + 1.62, self.z])
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                > 36.0
            {
                self.mechanic_bot.last_unavailable_reason = Some("outside_received_pose_reach");
                return Ok(false);
            }
        }
        if kind == "break_block" && !self.creative {
            return Ok(false);
        }
        if kind == "close_container" && self.mechanic_bot.menu.is_none() {
            self.mechanic_bot.last_unavailable_reason = Some("no_received_open_menu");
            return Ok(false);
        }
        if kind == "respawn" && (self.respawn_pending || self.last_health > 0.0) {
            return Ok(false);
        }
        // Admission is independent of transport and completion.
        shared.lock().actions[receipt].admitted = 1;
        match kind.as_str() {
            "reader_rate_limit" | "restore_reader" => {
                let rate = if kind == "reader_rate_limit" {
                    let state = shared.lock();
                    state.spec.actions[state.actions[receipt].action].bytes_per_second
                } else {
                    None
                };
                self.reader_control
                    .as_ref()
                    .context("reader relay not installed")?
                    .send(rate)
                    .context("reader relay closed")?;
                let mut state = shared.lock();
                state.players[self.index as usize].reader_bytes_per_second = rate;
                // This completes the local control change, not convergence.
                state.actions[receipt].completed = 1;
                state.actions[receipt].completed_in_window = self
                    .current_window()
                    .map(|index| self.schedule.windows[index].name.clone());
            }
            "open_container" => {
                if let Some((id, _, _)) = self.mechanic_bot.menu.take() {
                    client
                        .write_packet(&ServerboundContainerClose { container_id: id })
                        .await?;
                }
                let Some(MechanicExpectation::Menu(index, _)) = expectation.as_ref() else {
                    unreachable!()
                };
                self.mechanic_bot.opening = Some(*index);
                self.mechanic_use_block(client, position.expect("position"))
                    .await?;
            }
            "use_block" => {
                self.mechanic_use_block(client, position.expect("position"))
                    .await?;
                if self.wire_trace_sink.is_some() {
                    self.trace_wire(
                        "button_use_written",
                        json!({
                        "action":shared.lock().actions[receipt].action,
                        "packet_id":ServerboundUseItemOn::ID,
                        "position":position,"use_sequence":self.sequence}),
                    )?;
                }
            }
            "break_block" | "drop_stack" => {
                self.sequence = self
                    .sequence
                    .checked_add(1)
                    .context("mechanic sequence overflow")?;
                let [x, y, z] = position.unwrap_or([0, 0, 0]);
                client
                    .write_packet(&ServerboundPlayerAction {
                        action: if kind == "break_block" {
                            PlayerActionKind::StartDestroyBlock
                        } else {
                            PlayerActionKind::DropAllItems
                        },
                        position: pack_block_pos(x, y, z),
                        direction: Direction::Up,
                        sequence: self.sequence,
                    })
                    .await?;
            }
            "close_container" => {
                let (id, _, _) = self
                    .mechanic_bot
                    .menu
                    .take()
                    .expect("menu admission checked");
                client
                    .write_packet(&ServerboundContainerClose { container_id: id })
                    .await?;
            }
            "click_slot" => {
                let (id, _, version) = self.mechanic_bot.menu.expect("received menu");
                client
                    .write_packet(&ServerboundContainerClick {
                        container_id: id,
                        state_id: version,
                        slot_num: slot.expect("slot"),
                        button_num: button,
                        container_input: if mode == 1 {
                            ContainerInput::QuickMove
                        } else {
                            ContainerInput::Pickup
                        },
                        changed_slots: Vec::new(),
                        carried_item: {
                            let state = shared.lock();
                            let (_, index, _) = self.mechanic_bot.menu.expect("received menu");
                            state
                                .predicted_carried(
                                    index,
                                    slot.expect("slot") as usize,
                                    button,
                                    mode,
                                )
                                .context("click prediction unavailable")?
                        },
                    })
                    .await?;
            }
            "attack_entity" | "entity_interact" => {
                let Some(MechanicExpectation::Entity(uuid, _, _, _)) = expectation.as_ref() else {
                    unreachable!()
                };
                let id = shared.lock().entities[uuid].entity_id;
                if kind == "attack_entity" {
                    client
                        .write_packet(&ServerboundAttack { entity_id: id })
                        .await?;
                } else {
                    client
                        .write_packet(&ServerboundInteract {
                            entity_id: id,
                            hand: InteractionHand::MainHand,
                            location: EntityVec3::ZERO,
                            using_secondary_action: false,
                        })
                        .await?;
                }
            }
            "use_item" => {
                self.sequence = self
                    .sequence
                    .checked_add(1)
                    .context("mechanic sequence overflow")?;
                client
                    .write_packet(&ServerboundUseItem {
                        hand: InteractionHand::MainHand,
                        sequence: self.sequence,
                        y_rot: yaw,
                        x_rot: pitch,
                    })
                    .await?;
            }
            "select_slot" => {
                let slot = slot.expect("slot");
                ensure!((0..9).contains(&slot), "hotbar slot outside 0..8");
                client
                    .write_packet(&ServerboundSetCarriedItem { slot })
                    .await?;
                self.mechanic_bot.selected_slot = slot as usize;
            }
            "respawn" => {
                client
                    .write_packet(&ServerboundClientCommand {
                        action: ClientCommandAction::PerformRespawn,
                    })
                    .await?;
                self.respawn_pending = true;
                self.metrics.respawn_requests += 1;
            }
            "set_direction" => {
                self.direction = direction.context("set_direction needs direction")?;
                let yaw = match self.direction {
                    0 => -90.0,
                    1 => 0.0,
                    2 => 90.0,
                    3 => 180.0,
                    _ => unreachable!("validated direction"),
                };
                client
                    .write_packet(&ServerboundMovePlayerRot {
                        yaw,
                        pitch,
                        flags: MovePlayerFlags::new(self.last_on_ground, false),
                    })
                    .await?;
                self.yaw = yaw;
                self.pitch = pitch;
            }
            "set_rotation" => {
                client
                    .write_packet(&ServerboundMovePlayerRot {
                        yaw,
                        pitch,
                        flags: MovePlayerFlags::new(self.last_on_ground, false),
                    })
                    .await?;
                self.yaw = yaw;
                self.pitch = pitch;
            }
            "command" => {
                let packet = ServerboundChatCommand {
                    command: command.expect("validated command"),
                };
                client.write_packet(&packet).await?;
                self.metrics.commands_sent += 1;
                self.mechanic_bot.last_command = Some((receipt, Instant::now()));
                if packet.command.starts_with("tp ") || packet.command.starts_with("teleport ") {
                    self.mechanic_bot.pose_sync_expected = true;
                }
                if self.wire_trace_sink.is_some() {
                    self.trace_wire(
                        "command_written",
                        json!({
                        "action":shared.lock().actions[receipt].action,
                        "packet_id":ServerboundChatCommand::ID,"command":packet.command,
                        "position":[self.x,self.y,self.z]}),
                    )?;
                }
            }
            _ => unreachable!("validated action"),
        }
        shared.lock().actions[receipt].writes_completed = 1;
        if let Some(expectation) = expectation {
            self.mechanic_bot.pending.push_back(PendingMechanic {
                receipt,
                expectation,
                sent: Instant::now(),
            });
        }
        Ok(true)
    }
    async fn mechanic_poll(&mut self, client: &mut Client, elapsed: Duration) -> Result<()> {
        let window = self
            .schedule
            .window_at(elapsed)
            .map(|i| self.schedule.windows[i].name.clone());
        self.complete_mechanics(window.as_deref());
        self.close_received_menu(client).await?;
        if self.mechanic_read_only {
            return Ok(());
        }
        self.release_held(client, elapsed).await?;
        let Some(shared) = self.mechanics.as_ref().map(Arc::clone) else {
            return Ok(());
        };
        let mut latest: Option<usize> = None;
        {
            let mut state = shared.lock();
            if state.spec.snapshot_only {
                return Ok(());
            }
            while let Some(&index) = self.mechanic_bot.assigned.get(self.mechanic_bot.cursor) {
                if u128::from(state.actions[index].at_ms) > elapsed.as_millis() {
                    break;
                }
                self.mechanic_bot.cursor += 1;
                if let Some(old) = latest {
                    state.actions[old].dropped_late = 1;
                    state.actions[old].consumed = true;
                }
                latest = Some(index);
            }
        }
        let Some(index) = latest else {
            return Ok(());
        };
        let allowed = {
            let mut state = shared.lock();
            let receipt = &mut state.actions[index];
            receipt.consumed = true;
            let wi = receipt.window_index;
            self.schedule.window_at(elapsed) == Some(wi)
                && (!self.schedule.windows[wi].recovery || receipt.kind == "restore_reader")
        };
        if !allowed {
            shared.lock().actions[index].dropped_late = 1;
            return Ok(());
        }
        let kind = shared.lock().actions[index].kind.clone();
        if self.wire_trace_sink.is_some() && kind == "use_block" {
            let state = shared.lock();
            self.trace_wire(
                "button_offer",
                json!({
                "action":state.actions[index].action,
                "declared_at_ms":state.actions[index].at_ms,
                "target":state.spec.actions[state.actions[index].action].position,
                "position":[self.x,self.y,self.z]}),
            )?;
        }
        if !matches!(
            kind.as_str(),
            "respawn" | "reader_rate_limit" | "restore_reader"
        ) && (self.respawn_pending || self.last_health <= 0.0)
        {
            shared.lock().actions[index].dropped_unavailable = 1;
            return Ok(());
        }
        if self.mechanic_bot.pending.len() >= ACTION_CAP {
            shared.lock().actions[index].dropped_capacity = 1;
            return Ok(());
        }
        // Menu-flow actions queue behind a held open so they run in order.
        if !self.mechanic_bot.held.is_empty() && is_menu_flow(&kind) {
            self.mechanic_bot.held.push_back(index);
            return Ok(());
        }
        self.mechanic_bot.last_unavailable_reason = None;
        match self.mechanic_action(client, index).await {
            Ok(true) => {}
            Ok(false) => {
                if self.hold_unavailable(index, &kind) {
                    return Ok(());
                }
                self.refuse_mechanic(index, elapsed)?;
            }
            Err(error) => {
                shared.lock().actions[index]
                    .errors
                    .push(format!("{error:#}"));
                return Err(error);
            }
        }
        Ok(())
    }
    /// Holds an action that is unavailable only until a server event arrives:
    /// the pose sync of this bot's own pending teleport, the menu of an open
    /// that is already in flight, or the completion of the previous click's
    /// transfer. Returns whether the action was held.
    fn hold_unavailable(&mut self, index: usize, kind: &str) -> bool {
        let reason = self.mechanic_bot.last_unavailable_reason;
        let pose = reason == Some("outside_received_pose_reach")
            && self.mechanic_bot.pose_sync_expected
            && self.mechanic_bot.held.is_empty();
        let transfer_pending = self
            .mechanic_bot
            .pending
            .iter()
            .any(|pending| matches!(&pending.expectation, MechanicExpectation::MenuChanged(_)));
        let menu = matches!(kind, "click_slot" | "close_container")
            && (self.mechanic_bot.opening.is_some() || transfer_pending);
        if !pose && !menu {
            return false;
        }
        if pose {
            self.mechanic_bot.held_pose_blocked = true;
            self.mechanic_bot.pose_synced = false;
        }
        self.mechanic_bot.held.push_back(index);
        true
    }
    /// Re-evaluates held actions in order. The pose-blocked head waits for the
    /// next received SynchronizePlayerPosition; menu dependents wait for the
    /// open's menu and content. Everything left is dropped at window end, and a
    /// finally unavailable head drops the actions queued behind it.
    async fn release_held(&mut self, client: &mut Client, elapsed: Duration) -> Result<()> {
        let Some(shared) = self.mechanics.as_ref().map(Arc::clone) else {
            return Ok(());
        };
        while let Some(&receipt) = self.mechanic_bot.held.front() {
            let window = shared.lock().actions[receipt].window_index;
            if self.schedule.window_at(elapsed) != Some(window) {
                self.drop_held("held_window_ended", elapsed)?;
                break;
            }
            if self.mechanic_bot.held_pose_blocked
                && !std::mem::take(&mut self.mechanic_bot.pose_synced)
            {
                break;
            }
            self.mechanic_bot.last_unavailable_reason = None;
            if self.mechanic_action(client, receipt).await? {
                self.mechanic_bot.held.pop_front();
                self.mechanic_bot.held_pose_blocked = false;
                continue;
            }
            if self.mechanic_bot.held_pose_blocked
                && !(self.mechanic_bot.last_unavailable_reason
                    == Some("outside_received_pose_reach")
                    && self.mechanic_bot.pose_sync_expected)
            {
                self.drop_held("held_open_dropped", elapsed)?;
            }
            break;
        }
        Ok(())
    }
    fn drop_held(&mut self, reason: &'static str, elapsed: Duration) -> Result<()> {
        self.mechanic_bot.held_pose_blocked = false;
        let mut head = true;
        while let Some(index) = self.mechanic_bot.held.pop_front() {
            // A dropped head keeps the reason it was last refused for.
            if reason == "held_window_ended" || !head {
                self.mechanic_bot.last_unavailable_reason = Some(reason);
            }
            head = false;
            self.refuse_mechanic(index, elapsed)?;
        }
        Ok(())
    }
    fn refuse_mechanic(&mut self, index: usize, elapsed: Duration) -> Result<()> {
        let Some(shared) = self.mechanics.as_ref().map(Arc::clone) else {
            return Ok(());
        };
        let mut state = shared.lock();
        state.actions[index].dropped_unavailable = 1;
        if self.wire_trace_sink.is_some() {
            self.trace_wire(
                "action_refused",
                json!({
                "action":state.actions[index].action,"kind":state.actions[index].kind,
                "reason":self.mechanic_bot.last_unavailable_reason,
                "position":[self.x,self.y,self.z],
                "target":state.spec.actions[state.actions[index].action].position}),
            )?;
        }
        let action = &state.spec.actions[state.actions[index].action];
        let now = Instant::now();
        let last_command = self.mechanic_bot.last_command.map(|(receipt, sent)| {
            let command = &state.spec.actions[state.actions[receipt].action];
            json!({"action":state.actions[receipt].action, "command":command.command,
                "age_ms":now.duration_since(sent).as_millis()})
        });
        let last_sync = self.mechanic_bot.last_position_sync.map(|(received, position)| {
            json!({"position":position, "age_ms":now.duration_since(received).as_millis()})
        });
        eprintln!(
            "{}",
            json!({"event":"mechanic_unavailable", "bot":self.index,
            "action":state.actions[index].action, "kind":action.kind,
            "declared_at_ms":state.actions[index].at_ms,
            "offered_elapsed_ms":elapsed.as_millis(),
            "reason":self.mechanic_bot.last_unavailable_reason,
            "position":[self.x,self.y,self.z], "target":action.position,
            "opening":self.mechanic_bot.opening, "menu":self.mechanic_bot.menu,
            "pending":self.mechanic_bot.pending.len(), "last_command":last_command,
            "last_position_sync":last_sync})
        );
        Ok(())
    }
}

/// Actions that depend on the container flow of an earlier open.
fn is_menu_flow(kind: &str) -> bool {
    matches!(kind, "open_container" | "click_slot" | "close_container")
}
