//! Deterministic intent merge (ADR 0013 "Tick" step 3).
//!
//! Every entity (and the input step for players) emits intents against
//! snapshot `t`. Each intent declares its complete footprint: the targets it
//! changes and the sources it consumes. Merge applies intents one at a time
//! in a fixed order (tick-stable actor id, then intent sequence) against the
//! accumulated merge state; an intent whose footprint no longer validates is
//! rejected whole. Intents with disjoint footprints may be applied in
//! parallel; the result equals the serial order.
//!
//! Inventory claims inside a footprint are data here; the `inventory` module
//! (owned separately) validates and applies them through [`InventoryLedger`].

use crate::boundary::{BlockPos, EntityId};

/// One validated inventory slot claim inside a footprint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvClaim {
    /// Owning player entity.
    pub owner: EntityId,
    /// Slot index in the owner's inventory.
    pub slot: u16,
    /// Expected item protocol id before the tick.
    pub expect_item: u32,
    /// Expected count before the tick.
    pub expect_count: i32,
    /// Count removed by this intent.
    pub take: i32,
    /// Item placed back (crafting result, container swap), if any.
    pub put_item: Option<u32>,
    /// Count placed back.
    pub put_count: i32,
}

/// Complete footprint of one intent. Validation is all-or-nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Footprint {
    /// Entities read (must still be alive).
    pub entity_reads: Vec<EntityId>,
    /// Entities mutated (must still be alive).
    pub entity_writes: Vec<EntityId>,
    /// Item entity consumed whole (pickup, merge). At most one winner.
    pub item_take: Option<EntityId>,
    /// `(position, expected state before the tick)` preconditions.
    pub block_expect: Vec<(BlockPos, u32)>,
    /// `(position, new state)` writes, applied on acceptance.
    pub block_write: Vec<(BlockPos, u32)>,
    /// Inventory slot claims, validated by the inventory module.
    pub inv: Vec<InvClaim>,
}

/// Hit context captured before the source can move or a projectile despawns.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DamageSource {
    Melee {
        attacker: EntityId,
        position: [f64; 3],
        yaw: f32,
        bonus_knockback: f64,
        sweep: bool,
        /// Captured before weapon wear or later inventory inputs change the stack.
        fire_ticks: u32,
        bane_ticks: u32,
    },
    /// Mace context captured before weapon wear; ordinary melee callers stay unchanged.
    Mace {
        attacker: EntityId,
        position: [f64; 3],
        yaw: f32,
        bonus_knockback: f64,
        fire_ticks: u32,
        bane_ticks: u32,
        attack: crate::combat::MaceAttack,
    },
    /// Sweep knockback follows the attacker's yaw, not the victim's position.
    Sweep {
        attacker: EntityId,
        primary: EntityId,
        position: [f64; 3],
        yaw: f32,
        fire_ticks: u32,
        bane_ticks: u32,
    },
    /// Horizontal flight velocity, not the owner's position.
    Projectile {
        projectile: EntityId,
        /// Retained before the direct entity can disappear; loot predicates
        /// distinguish arrows, fireballs, skulls and other projectile types.
        direct_type_id: u32,
        owner: EntityId,
        velocity: [f64; 2],
        punch: u32,
        flame: bool,
    },
    /// Explosion attribution; blast motion and shield semantics remain owned
    /// by the explosion pipeline rather than ordinary attack knockback.
    Explosion {
        source: EntityId,
        /// Captured while the living source still exists (creepers leave before merge).
        kill_credit: Option<(EntityId, u32)>,
    },
}

/// What the intent does once its footprint validates.
#[derive(Debug, Clone, PartialEq)]
pub enum IntentKind {
    /// A client approval is validated and consumed only at the actual merge,
    /// after later inputs and the entity phase can no longer change its context.
    AuthorizedBuild {
        authorization: crate::build_precommit::AuthorizedBuild,
    },
    /// Claim a due custom attack's exact native equipment and resources at merge.
    PhysicalAttack {
        started_tick: u64,
    },
    /// A custom projectile retains its admitted physical impact descriptor.
    PhysicalDamageEntity {
        target: EntityId,
        impact: crate::physical_defense::PhysicalImpact,
        source: Option<DamageSource>,
    },
    /// Deal `amount` damage to `target`.
    DamageEntity {
        target: EntityId,
        amount: f32,
        /// How armor and enchantments reduce it.
        kind: crate::survival::DamageKind,
        source: Option<DamageSource>,
    },
    /// Take `count` items from an item entity.
    PickupItem {
        item: EntityId,
        count: i32,
    },
    /// Break the block (footprint carries expect + write air).
    BreakBlock {
        pos: BlockPos,
    },
    /// Place the block (footprint carries expect + write state).
    PlaceBlock {
        pos: BlockPos,
        state: u32,
    },
    /// Spawn a child entity at a position.
    SpawnChild {
        pos: [f64; 3],
        type_id: u32,
        health: f32,
    },
    /// Consume both parents' love, apply cooldown, and stage their child.
    Breed {
        partner: EntityId,
        child_pos: [f64; 3],
        child_type: u32,
    },
    /// Feed breeding food: `target` falls in love (the footprint's
    /// inventory claim takes the food).
    FeedAnimal {
        target: EntityId,
    },
    DolphinFeed {
        plan: crate::dolphin::FeedPlan,
    },
    /// Hand use of an interactive block (door, trapdoor, fence gate,
    /// lever): the footprint carries the expected and toggled states.
    UseBlock {
        pos: BlockPos,
        hand: Option<crate::boundary::Hand>,
    },
    /// Native animal harvesting; the exact held stack is checked at merge.
    HarvestAnimal {
        plan: crate::entity_life::HarvestPlan,
    },
    MountInteract {
        plan: crate::mounts::Plan,
    },
    /// Fill or empty a bucket: the footprint rewrites the fluid cell and
    /// swaps the held bucket; `give` is the new bucket when the held
    /// stack had more than one (it goes to the inventory instead).
    Bucket {
        pos: BlockPos,
        give: Option<u32>,
    },
    /// Till soil with a hoe or grow a crop with bone meal: the footprint
    /// rewrites the target (and takes the bone meal); `wear` is the damage
    /// the hoe takes on acceptance.
    Cultivate {
        wear: i32,
    },
    /// Successful composter fill; the debit and level change commit together.
    ComposterFill {
        pos: BlockPos,
        maturing: bool,
    },
    /// A creeper-style blast of `power` at `center` (vanilla
    /// `Level.explode` with block interaction): breaks blocks by their blast
    /// resistance and hurts entities by distance and exposure (damage scaled
    /// by world difficulty). The footprint is empty: the blast applies
    /// whole at merge, to whatever stands there then.
    Explode {
        center: [f64; 3],
        power: f32,
        /// Preserved even when the exploding mob leaves before merge.
        mob_source: bool,
        kill_credit: Option<(EntityId, u32)>,
    },
    /// A bed/anchor blast: atomically remove its claimed block cells first,
    /// then hurt every nearby entity, including the triggering player.
    EnvironmentalExplosion {
        center: [f64; 3],
        power: f32,
    },
    /// Select a charged anchor, after its block precondition validates.
    SetRespawn {
        pos: BlockPos,
    },
}

/// One intent from one actor in one tick.
#[derive(Debug, Clone, PartialEq)]
pub struct Intent {
    /// Tick-stable actor: wire entity id.
    pub actor: EntityId,
    /// Per-actor sequence within the tick.
    pub seq: u32,
    /// Effect on acceptance.
    pub kind: IntentKind,
    /// Complete footprint; validated whole.
    pub footprint: Footprint,
}

impl Intent {
    /// Fixed merge order: actor id, then sequence.
    pub fn order_key(&self) -> (EntityId, u32) {
        (self.actor, self.seq)
    }
}

/// Inventory state access for footprint validation. Implemented by the
/// `inventory` module over authoritative player inventories.
pub trait InventoryLedger {
    /// Whether every claim in `claims` validates against current state.
    fn check(&self, claims: &[InvClaim]) -> bool;
    /// Apply validated claims. Called only after [`InventoryLedger::check`].
    fn apply(&mut self, claims: &[InvClaim]);
}

/// No-op ledger for tests without inventories.
#[derive(Debug, Default)]
pub struct EmptyLedger;

impl InventoryLedger for EmptyLedger {
    fn check(&self, claims: &[InvClaim]) -> bool {
        claims.is_empty()
    }

    fn apply(&mut self, _claims: &[InvClaim]) {}
}

/// Sort intents into merge order (stable).
pub fn sort_intents(intents: &mut [Intent]) {
    intents.sort_by_key(Intent::order_key);
}

/// Whether two footprints touch disjoint state and may merge in parallel.
pub fn footprints_disjoint(a: &Footprint, b: &Footprint) -> bool {
    if a.item_take.is_some() && a.item_take == b.item_take {
        return false;
    }
    if a.entity_writes.iter().any(|id| {
        b.entity_writes.contains(id) || b.entity_reads.contains(id) || Some(*id) == b.item_take
    }) {
        return false;
    }
    if b.entity_writes
        .iter()
        .any(|id| a.entity_reads.contains(id) || Some(*id) == a.item_take)
    {
        return false;
    }
    if a.block_write
        .iter()
        .any(|(pos, _)| b.block_write.iter().any(|(other, _)| pos == other))
    {
        return false;
    }
    if a.inv.iter().any(|claim| {
        b.inv
            .iter()
            .any(|other| claim.owner == other.owner && claim.slot == other.slot)
    }) {
        return false;
    }
    true
}
