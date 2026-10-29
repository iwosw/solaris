//! Dense vanilla entity-type facts for Java Edition 26.3.
//!
//! Registry names and protocol IDs come from the 26.3 data-generator
//! `registries.json` report. The remaining scalar facts were independently
//! transcribed from the registration builder values and defaults in the local
//! clean-room oracle. No fallback row exists: unknown IDs and names stay
//! unknown. Validation and public access live in [`crate::entity_types`].
//! Behavior facts come from the constructor targets in `EntityType`, the
//! `DefaultAttributes` supplier map, entity inheritance and riding overrides,
//! and spawn packet construction/recreation methods in the same local oracle.

pub const MINECRAFT_VERSION: &str = "26.3";
pub const ENTITY_TYPE_COUNT: usize = 161;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct EntityDimensions {
    pub(crate) width: f32,
    pub(crate) height: f32,
    pub(crate) eye_height: f32,
    pub(crate) spawn_dimensions_scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MobCategory {
    Monster,
    Creature,
    Ambient,
    Axolotls,
    UndergroundWaterCreature,
    WaterCreature,
    WaterAmbient,
    Misc,
}

impl MobCategory {
    #[must_use]
    pub const fn max_instances_per_chunk(self) -> i16 {
        match self {
            Self::Monster => 70,
            Self::Creature => 10,
            Self::Ambient => 15,
            Self::Axolotls | Self::UndergroundWaterCreature | Self::WaterCreature => 5,
            Self::WaterAmbient => 20,
            Self::Misc => -1,
        }
    }

    #[must_use]
    pub const fn is_friendly(self) -> bool {
        !matches!(self, Self::Monster)
    }

    #[must_use]
    pub const fn is_persistent(self) -> bool {
        matches!(self, Self::Creature | Self::Misc)
    }

    #[must_use]
    pub const fn no_despawn_distance(self) -> u16 {
        32
    }

    #[must_use]
    pub const fn despawn_distance(self) -> u16 {
        match self {
            Self::WaterAmbient => 64,
            _ => 128,
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityTypeFlags(u8);

impl EntityTypeFlags {
    pub const SERIALIZABLE: Self = Self(1 << 0);
    pub const SUMMONABLE: Self = Self(1 << 1);
    pub const FIRE_IMMUNE: Self = Self(1 << 2);
    pub const CAN_SPAWN_FAR_FROM_PLAYER: Self = Self(1 << 3);
    pub const ALLOWED_IN_PEACEFUL: Self = Self(1 << 4);

    #[must_use]
    pub const fn contains(self, flag: Self) -> bool {
        self.0 & flag.0 == flag.0
    }

    #[must_use]
    pub const fn is_serializable(self) -> bool {
        self.contains(Self::SERIALIZABLE)
    }

    #[must_use]
    pub const fn is_summonable(self) -> bool {
        self.contains(Self::SUMMONABLE)
    }

    #[must_use]
    pub const fn is_fire_immune(self) -> bool {
        self.contains(Self::FIRE_IMMUNE)
    }

    #[must_use]
    pub const fn can_spawn_far_from_player(self) -> bool {
        self.contains(Self::CAN_SPAWN_FAR_FROM_PLAYER)
    }

    #[must_use]
    pub const fn is_allowed_in_peaceful(self) -> bool {
        self.contains(Self::ALLOWED_IN_PEACEFUL)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityArchetype {
    NonLiving,
    Living,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProjectileKind {
    None,
    Arrow,
    ThrowableItem,
    Hurting,
    WindCharge,
    ShulkerBullet,
    LlamaSpit,
    FireworkRocket,
    FishingBobber,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VehiclePassengerCapabilities {
    /// Maximum accepted riders when dynamic conditions such as fluid state permit it.
    /// Zero also covers entity types rejected as vehicles by the serialization gate.
    pub max_passengers: u8,
    /// Whether the entity's type-level riding rule can permit it to be a passenger.
    pub can_be_passenger: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PhysicalSimulationClass {
    Immobile,
    LivingGround,
    LivingFlying,
    LivingAquatic,
    LivingAmphibious,
    LivingAttached,
    Projectile,
    Boat,
    Minecart,
    Item,
    ExperienceOrb,
    FallingBlock,
    PrimedTnt,
    EyeOfEnder,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DefaultAttributeTemplateIdentity {
    None,
    Vanilla(&'static str),
}

impl DefaultAttributeTemplateIdentity {
    #[must_use]
    pub const fn as_str(self) -> Option<&'static str> {
        match self {
            Self::None => None,
            Self::Vanilla(identity) => Some(identity),
        }
    }
}

#[repr(transparent)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MetadataSchemaIdentity(&'static str);

impl MetadataSchemaIdentity {
    const fn new(identity: &'static str) -> Self {
        Self(identity)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SpawnDataCategory {
    /// The add-entity packet carries a zero data value.
    Zero,
    /// Projectile owner entity ID, or zero when there is no owner.
    ProjectileOwner,
    /// Fishing-hook owner entity ID, or the hook's own entity ID when ownerless.
    ProjectileOwnerOrSelf,
    FallingBlockState,
    HangingDirection,
    WardenEmerging,
    /// Vanilla rejects add-entity packet construction for this instance.
    NeverSent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EntityInstanceCategory {
    Registered,
    /// An `EnderDragonPart` uses the ender-dragon `EntityType` internally but
    /// is not a registry instance and rejects add-entity packet construction.
    NonRegisteredDragonPart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityInstanceContract {
    pub category: EntityInstanceCategory,
    pub metadata_schema: MetadataSchemaIdentity,
    pub spawn_data: SpawnDataCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EntityBehaviorContract {
    pub archetype: EntityArchetype,
    pub projectile: ProjectileKind,
    pub vehicle_passenger: VehiclePassengerCapabilities,
    pub physical_simulation: PhysicalSimulationClass,
    pub default_attributes: DefaultAttributeTemplateIdentity,
    pub metadata_schema: MetadataSchemaIdentity,
    pub spawn_data: SpawnDataCategory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClassifiedEntityBehavior {
    protocol_id: u32,
    name: &'static str,
    behavior: EntityBehaviorContract,
}

macro_rules! default_attribute_template {
    (None) => {
        DefaultAttributeTemplateIdentity::None
    };
    ($identity:literal) => {
        DefaultAttributeTemplateIdentity::Vanilla($identity)
    };
}

// Row fields: protocol ID, canonical name, archetype, projectile kind,
// physical simulation, maximum passengers, can be a passenger, default
// attribute supplier identity, concrete metadata schema, spawn packet data.
macro_rules! define_entity_behaviors {
    ($(
        $protocol_id:literal, $name:literal, $archetype:ident, $projectile:ident,
        $physical:ident, $max_passengers:literal, $can_be_passenger:literal,
        $attributes:tt, $metadata:literal, $spawn_data:ident;
    )+) => {
        static ENTITY_BEHAVIORS: [ClassifiedEntityBehavior; ENTITY_TYPE_COUNT] = [$(
            ClassifiedEntityBehavior {
                protocol_id: $protocol_id,
                name: $name,
                behavior: EntityBehaviorContract {
                    archetype: EntityArchetype::$archetype,
                    projectile: ProjectileKind::$projectile,
                    vehicle_passenger: VehiclePassengerCapabilities {
                        max_passengers: $max_passengers,
                        can_be_passenger: $can_be_passenger,
                    },
                    physical_simulation: PhysicalSimulationClass::$physical,
                    default_attributes: default_attribute_template!($attributes),
                    metadata_schema: MetadataSchemaIdentity::new($metadata),
                    spawn_data: SpawnDataCategory::$spawn_data,
                },
            }
        ),+];
    };
}

define_entity_behaviors! {
    0, "minecraft:acacia_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    1, "minecraft:acacia_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    2, "minecraft:allay", Living, None, LivingFlying, 1, true, "Allay.createAttributes", "Allay", Zero;
    3, "minecraft:area_effect_cloud", NonLiving, None, Immobile, 1, true, None, "AreaEffectCloud", Zero;
    4, "minecraft:armadillo", Living, None, LivingGround, 1, true, "Armadillo.createAttributes", "Armadillo", Zero;
    5, "minecraft:armor_stand", Living, None, LivingGround, 1, true, "ArmorStand.createAttributes", "ArmorStand", Zero;
    6, "minecraft:arrow", NonLiving, Arrow, Projectile, 1, true, None, "Arrow", ProjectileOwner;
    7, "minecraft:axolotl", Living, None, LivingAmphibious, 1, true, "Axolotl.createAttributes", "Axolotl", Zero;
    8, "minecraft:bamboo_chest_raft", NonLiving, None, Boat, 1, true, None, "ChestRaft", Zero;
    9, "minecraft:bamboo_raft", NonLiving, None, Boat, 2, true, None, "Raft", Zero;
    10, "minecraft:bat", Living, None, LivingFlying, 1, true, "Bat.createAttributes", "Bat", Zero;
    11, "minecraft:bee", Living, None, LivingFlying, 1, true, "Bee.createAttributes", "Bee", Zero;
    12, "minecraft:birch_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    13, "minecraft:birch_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    14, "minecraft:blaze", Living, None, LivingFlying, 1, true, "Blaze.createAttributes", "Blaze", Zero;
    15, "minecraft:block_display", NonLiving, None, Immobile, 1, true, None, "Display.BlockDisplay", Zero;
    16, "minecraft:bogged", Living, None, LivingGround, 1, true, "Bogged.createAttributes", "Bogged", Zero;
    17, "minecraft:breeze", Living, None, LivingGround, 1, true, "Breeze.createAttributes", "Breeze", Zero;
    18, "minecraft:breeze_wind_charge", NonLiving, WindCharge, Projectile, 1, true, None, "BreezeWindCharge", ProjectileOwner;
    19, "minecraft:camel", Living, None, LivingGround, 3, true, "Camel.createAttributes", "Camel", Zero;
    20, "minecraft:camel_husk", Living, None, LivingGround, 3, true, "Camel.createAttributes", "CamelHusk", Zero;
    21, "minecraft:cat", Living, None, LivingGround, 1, true, "Cat.createAttributes", "Cat", Zero;
    22, "minecraft:cave_spider", Living, None, LivingGround, 1, true, "CaveSpider.createCaveSpider", "CaveSpider", Zero;
    23, "minecraft:cherry_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    24, "minecraft:cherry_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    25, "minecraft:chest_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartChest", Zero;
    26, "minecraft:chicken", Living, None, LivingGround, 1, true, "Chicken.createAttributes", "Chicken", Zero;
    27, "minecraft:cod", Living, None, LivingAquatic, 1, true, "AbstractFish.createAttributes", "Cod", Zero;
    28, "minecraft:copper_golem", Living, None, LivingGround, 1, true, "CopperGolem.createAttributes", "CopperGolem", Zero;
    29, "minecraft:command_block_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartCommandBlock", Zero;
    30, "minecraft:cow", Living, None, LivingGround, 1, true, "Cow.createAttributes", "Cow", Zero;
    31, "minecraft:creaking", Living, None, LivingGround, 1, true, "Creaking.createAttributes", "Creaking", Zero;
    32, "minecraft:creeper", Living, None, LivingGround, 1, true, "Creeper.createAttributes", "Creeper", Zero;
    33, "minecraft:cushion", NonLiving, None, Immobile, 1, true, None, "Cushion", Zero;
    34, "minecraft:dark_oak_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    35, "minecraft:dark_oak_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    36, "minecraft:dolphin", Living, None, LivingAquatic, 1, true, "Dolphin.createAttributes", "Dolphin", Zero;
    37, "minecraft:donkey", Living, None, LivingGround, 1, true, "AbstractChestedHorse.createBaseChestedHorseAttributes", "Donkey", Zero;
    38, "minecraft:dragon_fireball", NonLiving, Hurting, Projectile, 1, true, None, "DragonFireball", ProjectileOwner;
    39, "minecraft:drowned", Living, None, LivingAmphibious, 1, true, "Drowned.createAttributes", "Drowned", Zero;
    40, "minecraft:egg", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownEgg", ProjectileOwner;
    41, "minecraft:elder_guardian", Living, None, LivingAquatic, 1, true, "ElderGuardian.createAttributes", "ElderGuardian", Zero;
    42, "minecraft:enderman", Living, None, LivingGround, 1, true, "EnderMan.createAttributes", "EnderMan", Zero;
    43, "minecraft:endermite", Living, None, LivingGround, 1, true, "Endermite.createAttributes", "Endermite", Zero;
    44, "minecraft:ender_dragon", Living, None, LivingFlying, 1, false, "EnderDragon.createAttributes", "EnderDragon", Zero;
    45, "minecraft:ender_pearl", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownEnderpearl", ProjectileOwner;
    46, "minecraft:end_crystal", NonLiving, None, Immobile, 1, true, None, "EndCrystal", Zero;
    47, "minecraft:evoker", Living, None, LivingGround, 1, true, "Evoker.createAttributes", "Evoker", Zero;
    48, "minecraft:evoker_fangs", NonLiving, None, Immobile, 1, true, None, "EvokerFangs", Zero;
    49, "minecraft:experience_bottle", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownExperienceBottle", ProjectileOwner;
    50, "minecraft:experience_orb", NonLiving, None, ExperienceOrb, 1, true, None, "ExperienceOrb", Zero;
    51, "minecraft:eye_of_ender", NonLiving, None, EyeOfEnder, 1, true, None, "EyeOfEnder", Zero;
    52, "minecraft:falling_block", NonLiving, None, FallingBlock, 1, true, None, "FallingBlockEntity", FallingBlockState;
    53, "minecraft:fireball", NonLiving, Hurting, Projectile, 1, true, None, "LargeFireball", ProjectileOwner;
    54, "minecraft:firework_rocket", NonLiving, FireworkRocket, Projectile, 1, true, None, "FireworkRocketEntity", ProjectileOwner;
    55, "minecraft:fox", Living, None, LivingGround, 1, true, "Fox.createAttributes", "Fox", Zero;
    56, "minecraft:frog", Living, None, LivingAmphibious, 1, true, "Frog.createAttributes", "Frog", Zero;
    57, "minecraft:furnace_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartFurnace", Zero;
    58, "minecraft:ghast", Living, None, LivingFlying, 1, true, "Ghast.createAttributes", "Ghast", Zero;
    59, "minecraft:happy_ghast", Living, None, LivingFlying, 4, true, "HappyGhast.createAttributes", "HappyGhast", Zero;
    60, "minecraft:giant", Living, None, LivingGround, 1, true, "Giant.createAttributes", "Giant", Zero;
    61, "minecraft:glow_item_frame", NonLiving, None, Immobile, 1, true, None, "GlowItemFrame", HangingDirection;
    62, "minecraft:glow_squid", Living, None, LivingAquatic, 1, true, "GlowSquid.createAttributes", "GlowSquid", Zero;
    63, "minecraft:goat", Living, None, LivingGround, 1, true, "Goat.createAttributes", "Goat", Zero;
    64, "minecraft:guardian", Living, None, LivingAquatic, 1, true, "Guardian.createAttributes", "Guardian", Zero;
    65, "minecraft:hoglin", Living, None, LivingGround, 1, true, "Hoglin.createAttributes", "Hoglin", Zero;
    66, "minecraft:hopper_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartHopper", Zero;
    67, "minecraft:horse", Living, None, LivingGround, 1, true, "AbstractHorse.createBaseHorseAttributes", "Horse", Zero;
    68, "minecraft:husk", Living, None, LivingGround, 1, true, "Zombie.createAttributes", "Husk", Zero;
    69, "minecraft:illusioner", Living, None, LivingGround, 1, true, "Illusioner.createAttributes", "Illusioner", Zero;
    70, "minecraft:interaction", NonLiving, None, Immobile, 1, true, None, "Interaction", Zero;
    71, "minecraft:iron_golem", Living, None, LivingGround, 1, true, "IronGolem.createAttributes", "IronGolem", Zero;
    72, "minecraft:item", NonLiving, None, Item, 1, true, None, "ItemEntity", Zero;
    73, "minecraft:item_display", NonLiving, None, Immobile, 1, true, None, "Display.ItemDisplay", Zero;
    74, "minecraft:item_frame", NonLiving, None, Immobile, 1, true, None, "ItemFrame", HangingDirection;
    75, "minecraft:jungle_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    76, "minecraft:jungle_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    77, "minecraft:leash_knot", NonLiving, None, Immobile, 0, true, None, "LeashFenceKnotEntity", Zero;
    78, "minecraft:lightning_bolt", NonLiving, None, Immobile, 0, true, None, "LightningBolt", Zero;
    79, "minecraft:llama", Living, None, LivingGround, 1, true, "Llama.createAttributes", "Llama", Zero;
    80, "minecraft:llama_spit", NonLiving, LlamaSpit, Projectile, 1, true, None, "LlamaSpit", ProjectileOwner;
    81, "minecraft:magma_cube", Living, None, LivingGround, 1, true, "MagmaCube.createAttributes", "MagmaCube", Zero;
    82, "minecraft:mangrove_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    83, "minecraft:mangrove_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    84, "minecraft:mannequin", Living, None, LivingGround, 1, true, "LivingEntity.createLivingAttributes", "Mannequin", Zero;
    85, "minecraft:marker", NonLiving, None, Immobile, 0, true, None, "Marker", NeverSent;
    86, "minecraft:minecart", NonLiving, None, Minecart, 1, true, None, "Minecart", Zero;
    87, "minecraft:mooshroom", Living, None, LivingGround, 1, true, "Cow.createAttributes", "MushroomCow", Zero;
    88, "minecraft:mule", Living, None, LivingGround, 1, true, "AbstractChestedHorse.createBaseChestedHorseAttributes", "Mule", Zero;
    89, "minecraft:nautilus", Living, None, LivingAquatic, 1, true, "Nautilus.createAttributes", "Nautilus", Zero;
    90, "minecraft:oak_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    91, "minecraft:oak_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    92, "minecraft:ocelot", Living, None, LivingGround, 1, true, "Ocelot.createAttributes", "Ocelot", Zero;
    93, "minecraft:ominous_item_spawner", NonLiving, None, Immobile, 0, true, None, "OminousItemSpawner", Zero;
    94, "minecraft:painting", NonLiving, None, Immobile, 1, true, None, "Painting", HangingDirection;
    95, "minecraft:pale_oak_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    96, "minecraft:pale_oak_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    97, "minecraft:panda", Living, None, LivingGround, 1, true, "Panda.createAttributes", "Panda", Zero;
    98, "minecraft:parched", Living, None, LivingGround, 1, true, "Parched.createAttributes", "Parched", Zero;
    99, "minecraft:parrot", Living, None, LivingFlying, 1, true, "Parrot.createAttributes", "Parrot", Zero;
    100, "minecraft:phantom", Living, None, LivingFlying, 1, true, "Monster.createMonsterAttributes", "Phantom", Zero;
    101, "minecraft:pig", Living, None, LivingGround, 1, true, "Pig.createAttributes", "Pig", Zero;
    102, "minecraft:piglin", Living, None, LivingGround, 1, true, "Piglin.createAttributes", "Piglin", Zero;
    103, "minecraft:piglin_brute", Living, None, LivingGround, 1, true, "PiglinBrute.createAttributes", "PiglinBrute", Zero;
    104, "minecraft:pillager", Living, None, LivingGround, 1, true, "Pillager.createAttributes", "Pillager", Zero;
    105, "minecraft:polar_bear", Living, None, LivingGround, 1, true, "PolarBear.createAttributes", "PolarBear", Zero;
    106, "minecraft:poplar_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    107, "minecraft:poplar_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    108, "minecraft:splash_potion", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownSplashPotion", ProjectileOwner;
    109, "minecraft:lingering_potion", NonLiving, ThrowableItem, Projectile, 1, true, None, "ThrownLingeringPotion", ProjectileOwner;
    110, "minecraft:pufferfish", Living, None, LivingAquatic, 1, true, "AbstractFish.createAttributes", "Pufferfish", Zero;
    111, "minecraft:rabbit", Living, None, LivingGround, 1, true, "Rabbit.createAttributes", "Rabbit", Zero;
    112, "minecraft:ravager", Living, None, LivingGround, 1, true, "Ravager.createAttributes", "Ravager", Zero;
    113, "minecraft:salmon", Living, None, LivingAquatic, 1, true, "AbstractFish.createAttributes", "Salmon", Zero;
    114, "minecraft:sheep", Living, None, LivingGround, 1, true, "Sheep.createAttributes", "Sheep", Zero;
    115, "minecraft:shulker", Living, None, LivingAttached, 1, true, "Shulker.createAttributes", "Shulker", Zero;
    116, "minecraft:shulker_bullet", NonLiving, ShulkerBullet, Projectile, 1, true, None, "ShulkerBullet", ProjectileOwner;
    117, "minecraft:silverfish", Living, None, LivingGround, 1, true, "Silverfish.createAttributes", "Silverfish", Zero;
    118, "minecraft:skeleton", Living, None, LivingGround, 1, true, "AbstractSkeleton.createAttributes", "Skeleton", Zero;
    119, "minecraft:skeleton_horse", Living, None, LivingGround, 1, true, "SkeletonHorse.createAttributes", "SkeletonHorse", Zero;
    120, "minecraft:slime", Living, None, LivingGround, 1, true, "Monster.createMonsterAttributes", "Slime", Zero;
    121, "minecraft:small_fireball", NonLiving, Hurting, Projectile, 1, true, None, "SmallFireball", ProjectileOwner;
    122, "minecraft:sniffer", Living, None, LivingGround, 1, true, "Sniffer.createAttributes", "Sniffer", Zero;
    123, "minecraft:snowball", NonLiving, ThrowableItem, Projectile, 1, true, None, "Snowball", ProjectileOwner;
    124, "minecraft:snow_golem", Living, None, LivingGround, 1, true, "SnowGolem.createAttributes", "SnowGolem", Zero;
    125, "minecraft:spawner_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartSpawner", Zero;
    126, "minecraft:spectral_arrow", NonLiving, Arrow, Projectile, 1, true, None, "SpectralArrow", ProjectileOwner;
    127, "minecraft:spider", Living, None, LivingGround, 1, true, "Spider.createAttributes", "Spider", Zero;
    128, "minecraft:spruce_boat", NonLiving, None, Boat, 2, true, None, "Boat", Zero;
    129, "minecraft:spruce_chest_boat", NonLiving, None, Boat, 1, true, None, "ChestBoat", Zero;
    130, "minecraft:squid", Living, None, LivingAquatic, 1, true, "Squid.createAttributes", "Squid", Zero;
    131, "minecraft:stray", Living, None, LivingGround, 1, true, "AbstractSkeleton.createAttributes", "Stray", Zero;
    132, "minecraft:strider", Living, None, LivingGround, 1, true, "Strider.createAttributes", "Strider", Zero;
    133, "minecraft:sulfur_cube", Living, None, LivingGround, 1, true, "SulfurCube.createSulfurCubeAttributes", "SulfurCube", Zero;
    134, "minecraft:tadpole", Living, None, LivingAquatic, 1, true, "Tadpole.createAttributes", "Tadpole", Zero;
    135, "minecraft:text_display", NonLiving, None, Immobile, 1, true, None, "Display.TextDisplay", Zero;
    136, "minecraft:tnt", NonLiving, None, PrimedTnt, 1, true, None, "PrimedTnt", Zero;
    137, "minecraft:tnt_minecart", NonLiving, None, Minecart, 1, true, None, "MinecartTNT", Zero;
    138, "minecraft:trader_llama", Living, None, LivingGround, 1, true, "Llama.createAttributes", "TraderLlama", Zero;
    139, "minecraft:trident", NonLiving, Arrow, Projectile, 1, true, None, "ThrownTrident", ProjectileOwner;
    140, "minecraft:tropical_fish", Living, None, LivingAquatic, 1, true, "AbstractFish.createAttributes", "TropicalFish", Zero;
    141, "minecraft:turtle", Living, None, LivingAmphibious, 1, true, "Turtle.createAttributes", "Turtle", Zero;
    142, "minecraft:vex", Living, None, LivingFlying, 1, true, "Vex.createAttributes", "Vex", Zero;
    143, "minecraft:villager", Living, None, LivingGround, 1, true, "Villager.createAttributes", "Villager", Zero;
    144, "minecraft:vindicator", Living, None, LivingGround, 1, true, "Vindicator.createAttributes", "Vindicator", Zero;
    145, "minecraft:wandering_trader", Living, None, LivingGround, 1, true, "Mob.createMobAttributes", "WanderingTrader", Zero;
    146, "minecraft:warden", Living, None, LivingGround, 1, false, "Warden.createAttributes", "Warden", WardenEmerging;
    147, "minecraft:wind_charge", NonLiving, WindCharge, Projectile, 1, true, None, "WindCharge", ProjectileOwner;
    148, "minecraft:witch", Living, None, LivingGround, 1, true, "Witch.createAttributes", "Witch", Zero;
    149, "minecraft:wither", Living, None, LivingFlying, 1, false, "WitherBoss.createAttributes", "WitherBoss", Zero;
    150, "minecraft:wither_skeleton", Living, None, LivingGround, 1, true, "AbstractSkeleton.createAttributes", "WitherSkeleton", Zero;
    151, "minecraft:wither_skull", NonLiving, Hurting, Projectile, 1, true, None, "WitherSkull", ProjectileOwner;
    152, "minecraft:wolf", Living, None, LivingGround, 1, true, "Wolf.createAttributes", "Wolf", Zero;
    153, "minecraft:zoglin", Living, None, LivingGround, 1, true, "Zoglin.createAttributes", "Zoglin", Zero;
    154, "minecraft:zombie", Living, None, LivingGround, 1, true, "Zombie.createAttributes", "Zombie", Zero;
    155, "minecraft:zombie_horse", Living, None, LivingGround, 1, true, "ZombieHorse.createAttributes", "ZombieHorse", Zero;
    156, "minecraft:zombie_nautilus", Living, None, LivingAquatic, 1, true, "ZombieNautilus.createAttributes", "ZombieNautilus", Zero;
    157, "minecraft:zombie_villager", Living, None, LivingGround, 1, true, "Zombie.createAttributes", "ZombieVillager", Zero;
    158, "minecraft:zombified_piglin", Living, None, LivingGround, 1, true, "ZombifiedPiglin.createAttributes", "ZombifiedPiglin", Zero;
    159, "minecraft:player", Living, None, LivingGround, 0, true, "Player.createAttributes", "Player", Zero;
    160, "minecraft:fishing_bobber", NonLiving, FishingBobber, Projectile, 0, true, None, "FishingHook", ProjectileOwnerOrSelf;
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityTypeContract {
    pub protocol_id: u32,
    pub name: &'static str,
    pub behavior: EntityBehaviorContract,
    pub(crate) dimensions: EntityDimensions,
    /// Vanilla tracking radius in chunks; the edge also caps it by view distance.
    pub tracking_range: u8,
    /// Vanilla movement update interval in ticks (appearance is not cadence-gated).
    pub update_interval: u32,
    pub(crate) category: MobCategory,
    pub(crate) flags: EntityTypeFlags,
}

impl EntityTypeContract {
    #[must_use]
    pub const fn mob_category(self) -> MobCategory {
        self.category
    }

    /// Classifies a normal instance created from this registered entity type.
    #[must_use]
    pub const fn registered_instance_contract(self) -> EntityInstanceContract {
        EntityInstanceContract {
            category: EntityInstanceCategory::Registered,
            metadata_schema: self.behavior.metadata_schema,
            spawn_data: self.behavior.spawn_data,
        }
    }
}

/// Classifies an `EnderDragonPart`, which is deliberately outside the 161-row
/// entity-type registry even though vanilla stores `EntityType.ENDER_DRAGON`
/// on each part instance.
#[must_use]
pub const fn ender_dragon_part_instance_contract_26_1_2() -> EntityInstanceContract {
    EntityInstanceContract {
        category: EntityInstanceCategory::NonRegisteredDragonPart,
        metadata_schema: MetadataSchemaIdentity::new("EnderDragonPart"),
        spawn_data: SpawnDataCategory::NeverSent,
    }
}

const FLAGS_HOSTILE: EntityTypeFlags = EntityTypeFlags(0x03);
const FLAGS_FIRE_HOSTILE: EntityTypeFlags = EntityTypeFlags(0x07);
const FLAGS_FAR_HOSTILE: EntityTypeFlags = EntityTypeFlags(0x0b);
const FLAGS_STANDARD: EntityTypeFlags = EntityTypeFlags(0x13);
const FLAGS_FIRE_STANDARD: EntityTypeFlags = EntityTypeFlags(0x17);
const FLAGS_RUNTIME_ONLY: EntityTypeFlags = EntityTypeFlags(0x18);
const FLAGS_TRANSIENT_SUMMONABLE: EntityTypeFlags = EntityTypeFlags(0x1a);
const FLAGS_FAR_STANDARD: EntityTypeFlags = EntityTypeFlags(0x1b);
const FLAGS_FIRE_FAR_STANDARD: EntityTypeFlags = EntityTypeFlags(0x1f);

macro_rules! eye_height {
    (default, $height:expr) => {
        $height * 0.85_f32
    };
    ($eye_height:literal, $height:expr) => {
        $eye_height
    };
}

// Row fields: protocol ID, name, width, height, eye height (or `default`),
// spawn-dimensions scale, tracking range, update interval, category, flags.
// The row form keeps the independently verified source representation
// auditable while emitting one simple immutable table.
macro_rules! define_entity_contract {
    ($(
        $protocol_id:literal, $name:literal, $width:literal, $height:literal,
        $eye_height:tt, $spawn_scale:literal, $tracking_range:literal,
        $update_interval:expr, $category:ident, $flags:ident;
    )+) => {
        static ENTITY_TYPES: [EntityTypeContract; ENTITY_TYPE_COUNT] = [$(
            EntityTypeContract {
                protocol_id: $protocol_id,
                name: $name,
                behavior: ENTITY_BEHAVIORS[$protocol_id].behavior,
                dimensions: EntityDimensions {
                    width: $width,
                    height: $height,
                    eye_height: eye_height!($eye_height, $height),
                    spawn_dimensions_scale: $spawn_scale,
                },
                tracking_range: $tracking_range,
                update_interval: $update_interval,
                category: MobCategory::$category,
                flags: $flags,
            }
        ),+];
    };
}

define_entity_contract! {
    0, "minecraft:acacia_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    1, "minecraft:acacia_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    2, "minecraft:allay", 0.35, 0.6, 0.36, 1.0, 8, 2, Creature, FLAGS_FAR_STANDARD;
    3, "minecraft:area_effect_cloud", 6.0, 0.5, default, 1.0, 10, i32::MAX as u32, Misc, FLAGS_FIRE_FAR_STANDARD;
    4, "minecraft:armadillo", 0.7, 0.65, 0.26, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    5, "minecraft:armor_stand", 0.5, 1.975, 1.7775, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    6, "minecraft:arrow", 0.5, 0.5, 0.13, 1.0, 4, 20, Misc, FLAGS_FAR_STANDARD;
    7, "minecraft:axolotl", 0.75, 0.42, 0.2751, 1.0, 10, 3, Axolotls, FLAGS_STANDARD;
    8, "minecraft:bamboo_chest_raft", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    9, "minecraft:bamboo_raft", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    10, "minecraft:bat", 0.5, 0.9, 0.45, 1.0, 5, 3, Ambient, FLAGS_STANDARD;
    11, "minecraft:bee", 0.55, 0.5, 0.3, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    12, "minecraft:birch_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    13, "minecraft:birch_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    14, "minecraft:blaze", 0.6, 1.8, default, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    15, "minecraft:block_display", 0.0, 0.0, default, 1.0, 10, 1, Misc, FLAGS_FAR_STANDARD;
    16, "minecraft:bogged", 0.6, 1.99, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    17, "minecraft:breeze", 0.6, 1.77, 1.3452, 1.0, 10, 3, Monster, FLAGS_HOSTILE;
    18, "minecraft:breeze_wind_charge", 0.3125, 0.3125, 0.0, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    19, "minecraft:camel", 1.7, 2.375, 2.275, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    20, "minecraft:camel_husk", 1.7, 2.375, 2.275, 1.0, 10, 3, Monster, FLAGS_STANDARD;
    21, "minecraft:cat", 0.6, 0.7, 0.35, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    22, "minecraft:cave_spider", 0.7, 0.5, 0.45, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    23, "minecraft:cherry_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    24, "minecraft:cherry_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    25, "minecraft:chest_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    26, "minecraft:chicken", 0.4, 0.7, 0.644, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    27, "minecraft:cod", 0.5, 0.3, 0.195, 1.0, 4, 3, WaterAmbient, FLAGS_STANDARD;
    28, "minecraft:copper_golem", 0.49, 0.98, 0.8125, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    29, "minecraft:command_block_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    30, "minecraft:cow", 0.9, 1.4, 1.3, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    31, "minecraft:creaking", 0.9, 2.7, 2.3, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    32, "minecraft:creeper", 0.6, 1.7, default, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    33, "minecraft:cushion", 1.0, 0.25, default, 1.0, 10, i32::MAX as u32, Misc, FLAGS_FAR_STANDARD;
    34, "minecraft:dark_oak_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    35, "minecraft:dark_oak_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    36, "minecraft:dolphin", 0.9, 0.6, 0.3, 1.0, 5, 3, WaterCreature, FLAGS_STANDARD;
    37, "minecraft:donkey", 1.3964844, 1.5, 1.425, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    38, "minecraft:dragon_fireball", 1.0, 1.0, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    39, "minecraft:drowned", 0.6, 1.95, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    40, "minecraft:egg", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    41, "minecraft:elder_guardian", 1.9975, 1.9975, 0.99875, 1.0, 10, 3, Monster, FLAGS_HOSTILE;
    42, "minecraft:enderman", 0.6, 2.9, 2.55, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    43, "minecraft:endermite", 0.4, 0.3, 0.13, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    44, "minecraft:ender_dragon", 16.0, 8.0, default, 1.0, 10, 3, Monster, FLAGS_FIRE_STANDARD;
    45, "minecraft:ender_pearl", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    46, "minecraft:end_crystal", 2.0, 2.0, default, 1.0, 16, i32::MAX as u32, Misc, FLAGS_FIRE_FAR_STANDARD;
    47, "minecraft:evoker", 0.6, 1.95, default, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    48, "minecraft:evoker_fangs", 0.5, 0.8, default, 1.0, 6, 2, Misc, FLAGS_FAR_STANDARD;
    49, "minecraft:experience_bottle", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    50, "minecraft:experience_orb", 0.5, 0.5, default, 1.0, 6, 20, Misc, FLAGS_FAR_STANDARD;
    51, "minecraft:eye_of_ender", 0.25, 0.25, default, 1.0, 4, 4, Misc, FLAGS_FAR_STANDARD;
    52, "minecraft:falling_block", 0.98, 0.98, default, 1.0, 10, 20, Misc, FLAGS_FAR_STANDARD;
    53, "minecraft:fireball", 1.0, 1.0, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    54, "minecraft:firework_rocket", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    55, "minecraft:fox", 0.6, 0.7, 0.4, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    56, "minecraft:frog", 0.5, 0.5, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    57, "minecraft:furnace_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    58, "minecraft:ghast", 4.0, 4.0, 2.6, 1.0, 10, 3, Monster, FLAGS_FIRE_HOSTILE;
    59, "minecraft:happy_ghast", 4.0, 4.0, 2.6, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    60, "minecraft:giant", 3.6, 12.0, 10.44, 1.0, 10, 3, Monster, FLAGS_HOSTILE;
    61, "minecraft:glow_item_frame", 0.5, 0.5, 0.0, 1.0, 10, i32::MAX as u32, Misc, FLAGS_FAR_STANDARD;
    62, "minecraft:glow_squid", 0.8, 0.8, 0.4, 1.0, 10, 3, UndergroundWaterCreature, FLAGS_STANDARD;
    63, "minecraft:goat", 0.9, 1.3, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    64, "minecraft:guardian", 0.85, 0.85, 0.425, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    65, "minecraft:hoglin", 1.3964844, 1.4, default, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    66, "minecraft:hopper_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    67, "minecraft:horse", 1.3964844, 1.6, 1.52, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    68, "minecraft:husk", 0.6, 1.95, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    69, "minecraft:illusioner", 0.6, 1.95, default, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    70, "minecraft:interaction", 0.0, 0.0, default, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    71, "minecraft:iron_golem", 1.4, 2.7, default, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    72, "minecraft:item", 0.25, 0.25, 0.2125, 1.0, 6, 20, Misc, FLAGS_FAR_STANDARD;
    73, "minecraft:item_display", 0.0, 0.0, default, 1.0, 10, 1, Misc, FLAGS_FAR_STANDARD;
    74, "minecraft:item_frame", 0.5, 0.5, 0.0, 1.0, 10, i32::MAX as u32, Misc, FLAGS_FAR_STANDARD;
    75, "minecraft:jungle_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    76, "minecraft:jungle_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    77, "minecraft:leash_knot", 0.375, 0.5, 0.0625, 1.0, 10, i32::MAX as u32, Misc, FLAGS_TRANSIENT_SUMMONABLE;
    78, "minecraft:lightning_bolt", 0.0, 0.0, default, 1.0, 16, i32::MAX as u32, Misc, FLAGS_TRANSIENT_SUMMONABLE;
    79, "minecraft:llama", 0.9, 1.87, 1.7765, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    80, "minecraft:llama_spit", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    81, "minecraft:magma_cube", 0.52, 0.52, 0.325, 4.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    82, "minecraft:mangrove_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    83, "minecraft:mangrove_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    84, "minecraft:mannequin", 0.6, 1.8, 1.62, 1.0, 32, 2, Misc, FLAGS_FAR_STANDARD;
    85, "minecraft:marker", 0.0, 0.0, default, 1.0, 0, 3, Misc, FLAGS_FAR_STANDARD;
    86, "minecraft:minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    87, "minecraft:mooshroom", 0.9, 1.4, 1.3, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    88, "minecraft:mule", 1.3964844, 1.6, 1.52, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    89, "minecraft:nautilus", 0.875, 0.95, 0.2751, 1.0, 10, 3, WaterCreature, FLAGS_STANDARD;
    90, "minecraft:oak_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    91, "minecraft:oak_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    92, "minecraft:ocelot", 0.6, 0.7, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    93, "minecraft:ominous_item_spawner", 0.25, 0.25, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    94, "minecraft:painting", 0.5, 0.5, default, 1.0, 10, i32::MAX as u32, Misc, FLAGS_FAR_STANDARD;
    95, "minecraft:pale_oak_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    96, "minecraft:pale_oak_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    97, "minecraft:panda", 1.3, 1.25, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    98, "minecraft:parched", 0.6, 1.99, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    99, "minecraft:parrot", 0.5, 0.9, 0.54, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    100, "minecraft:phantom", 0.9, 0.5, 0.175, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    101, "minecraft:pig", 0.9, 0.9, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    102, "minecraft:piglin", 0.6, 1.95, 1.79, 1.0, 8, 3, Monster, FLAGS_STANDARD;
    103, "minecraft:piglin_brute", 0.6, 1.95, 1.79, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    104, "minecraft:pillager", 0.6, 1.95, default, 1.0, 8, 3, Monster, FLAGS_FAR_HOSTILE;
    105, "minecraft:polar_bear", 1.4, 1.4, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    106, "minecraft:poplar_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    107, "minecraft:poplar_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    108, "minecraft:splash_potion", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    109, "minecraft:lingering_potion", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    110, "minecraft:pufferfish", 0.7, 0.7, 0.455, 1.0, 4, 3, WaterAmbient, FLAGS_STANDARD;
    111, "minecraft:rabbit", 0.49, 0.6, 0.59, 1.0, 8, 3, Creature, FLAGS_FAR_STANDARD;
    112, "minecraft:ravager", 1.95, 2.2, default, 1.0, 10, 3, Monster, FLAGS_HOSTILE;
    113, "minecraft:salmon", 0.7, 0.4, 0.26, 1.0, 4, 3, WaterAmbient, FLAGS_STANDARD;
    114, "minecraft:sheep", 0.9, 1.3, 1.235, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    115, "minecraft:shulker", 1.0, 1.0, 0.5, 1.0, 10, 3, Monster, FLAGS_FIRE_FAR_STANDARD;
    116, "minecraft:shulker_bullet", 0.3125, 0.3125, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    117, "minecraft:silverfish", 0.4, 0.3, 0.13, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    118, "minecraft:skeleton", 0.6, 1.99, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    119, "minecraft:skeleton_horse", 1.3964844, 1.6, 1.52, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    120, "minecraft:slime", 0.52, 0.52, 0.325, 4.0, 10, 3, Monster, FLAGS_HOSTILE;
    121, "minecraft:small_fireball", 0.3125, 0.3125, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    122, "minecraft:sniffer", 1.9, 1.75, 1.05, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    123, "minecraft:snowball", 0.25, 0.25, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    124, "minecraft:snow_golem", 0.7, 1.9, 1.7, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    125, "minecraft:spawner_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    126, "minecraft:spectral_arrow", 0.5, 0.5, 0.13, 1.0, 4, 20, Misc, FLAGS_FAR_STANDARD;
    127, "minecraft:spider", 1.4, 0.9, 0.65, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    128, "minecraft:spruce_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    129, "minecraft:spruce_chest_boat", 1.375, 0.5625, 0.5625, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    130, "minecraft:squid", 0.8, 0.8, 0.4, 1.0, 8, 3, WaterCreature, FLAGS_STANDARD;
    131, "minecraft:stray", 0.6, 1.99, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    132, "minecraft:strider", 0.9, 1.7, default, 1.0, 10, 3, Creature, FLAGS_FIRE_FAR_STANDARD;
    133, "minecraft:sulfur_cube", 0.49, 0.49, 0.175, 2.0, 10, 3, Monster, FLAGS_STANDARD;
    134, "minecraft:tadpole", 0.4, 0.3, 0.19500001, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    135, "minecraft:text_display", 0.0, 0.0, default, 1.0, 10, 1, Misc, FLAGS_FAR_STANDARD;
    136, "minecraft:tnt", 0.98, 0.98, 0.15, 1.0, 10, 10, Misc, FLAGS_FIRE_FAR_STANDARD;
    137, "minecraft:tnt_minecart", 0.98, 0.7, default, 1.0, 8, 3, Misc, FLAGS_FAR_STANDARD;
    138, "minecraft:trader_llama", 0.9, 1.87, 1.7765, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    139, "minecraft:trident", 0.5, 0.5, 0.13, 1.0, 4, 20, Misc, FLAGS_FAR_STANDARD;
    140, "minecraft:tropical_fish", 0.5, 0.4, 0.26, 1.0, 4, 3, WaterAmbient, FLAGS_STANDARD;
    141, "minecraft:turtle", 1.2, 0.4, default, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    142, "minecraft:vex", 0.4, 0.8, 0.51875, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    143, "minecraft:villager", 0.6, 1.95, 1.62, 1.0, 10, 3, Misc, FLAGS_FAR_STANDARD;
    144, "minecraft:vindicator", 0.6, 1.95, default, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    145, "minecraft:wandering_trader", 0.6, 1.95, 1.62, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    146, "minecraft:warden", 0.9, 2.9, default, 1.0, 16, 3, Monster, FLAGS_FIRE_HOSTILE;
    147, "minecraft:wind_charge", 0.3125, 0.3125, 0.0, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    148, "minecraft:witch", 0.6, 1.95, 1.62, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    149, "minecraft:wither", 0.9, 3.5, default, 1.0, 10, 3, Monster, FLAGS_FIRE_HOSTILE;
    150, "minecraft:wither_skeleton", 0.7, 2.4, 2.1, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    151, "minecraft:wither_skull", 0.3125, 0.3125, default, 1.0, 4, 10, Misc, FLAGS_FAR_STANDARD;
    152, "minecraft:wolf", 0.6, 0.85, 0.68, 1.0, 10, 3, Creature, FLAGS_FAR_STANDARD;
    153, "minecraft:zoglin", 1.3964844, 1.4, default, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    154, "minecraft:zombie", 0.6, 1.95, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    155, "minecraft:zombie_horse", 1.3964844, 1.6, 1.52, 1.0, 10, 3, Monster, FLAGS_STANDARD;
    156, "minecraft:zombie_nautilus", 0.875, 0.95, 0.2751, 1.0, 10, 3, Monster, FLAGS_STANDARD;
    157, "minecraft:zombie_villager", 0.6, 1.95, 1.74, 1.0, 8, 3, Monster, FLAGS_HOSTILE;
    158, "minecraft:zombified_piglin", 0.6, 1.95, 1.79, 1.0, 8, 3, Monster, FLAGS_FIRE_HOSTILE;
    159, "minecraft:player", 0.6, 1.8, 1.62, 1.0, 32, 2, Misc, FLAGS_RUNTIME_ONLY;
    160, "minecraft:fishing_bobber", 0.25, 0.25, default, 1.0, 4, 5, Misc, FLAGS_RUNTIME_ONLY;
}

#[inline]
#[must_use]
pub(crate) fn by_protocol_id(protocol_id: u32) -> Option<EntityTypeContract> {
    let index = usize::try_from(protocol_id).ok()?;
    ENTITY_TYPES.get(index).copied()
}

#[must_use]
pub(crate) fn by_name(name: &str) -> Option<EntityTypeContract> {
    ENTITY_TYPES
        .iter()
        .find(|candidate| candidate.name == name)
        .copied()
}

#[inline]
#[must_use]
pub fn entity_type_contract_26_1_2_by_protocol_id(protocol_id: u32) -> Option<EntityTypeContract> {
    by_protocol_id(protocol_id)
}

#[must_use]
pub fn entity_type_contract_26_1_2_by_name(name: &str) -> Option<EntityTypeContract> {
    by_name(name)
}

pub(crate) fn entity_type_contracts_26_1_2()
-> impl ExactSizeIterator<Item = EntityTypeContract> + DoubleEndedIterator + Clone {
    ENTITY_TYPES.iter().copied()
}

#[cfg(test)]
#[path = "entity_contract_26_1_2_tests.rs"]
mod tests;
