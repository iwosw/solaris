pub(super) struct OwnershipRule {
    pub(super) name: &'static str,
    pub(super) module_file: &'static str,
    pub(super) parent_file: &'static str,
    pub(super) mod_declaration: &'static str,
    pub(super) definition_file: &'static str,
    pub(super) definition_anchor: &'static str,
}

pub(super) const LOWER_CRATE_OWNERSHIP: &[OwnershipRule] = &[OwnershipRule {
    name: "plant rules",
    module_file: "crates/mc-world/src/plant_rules_26_1_2.rs",
    parent_file: "crates/mc-world/src/lib.rs",
    mod_declaration: "pub mod plant_rules_26_1_2;",
    definition_file: "crates/mc-world/src/plant_rules_26_1_2.rs",
    definition_anchor: "pub trait PlantBlockRead",
}];
