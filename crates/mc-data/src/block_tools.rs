//! Immutable plugin-authored block-tool rules: what a held plugin-owned item does
//! to the block it is used on. Rules are data, never policy: the native tick
//! applies the replacement, wear and grant atomically, and a rule only chooses
//! them. This module owns the strict JSON shape and its syntactic bounds; the
//! block registry is not known here, so registered-name and property checks run
//! where the registry exists (`mc-core` startup admission).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Identifier;

/// Most rules one plugin may author.
pub const MAX_BLOCK_TOOL_RULES: usize = 1024;
/// Most JSON bytes all of one plugin's rules may total.
pub const MAX_BLOCK_TOOL_BYTES: usize = 1_048_576;
/// Most wear one use may charge (the item durability bound).
pub const MAX_BLOCK_TOOL_WEAR: u32 = 32_767;
/// A `set` value that is replaced by the name of the clicked face.
pub const CLICKED_FACE: &str = "$face";
/// A `set` value starting with this prefix copies the named source property.
pub const COPY_PREFIX: char = '@';

/// Which clicked face a rule applies to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFace {
    /// Any of the six faces.
    Any,
    /// The top face only.
    Up,
    /// The bottom face only.
    Down,
    /// Any of the four horizontal faces.
    Side,
}

/// One rule: `item` used on `block` (whose properties include `when`) at a
/// matching `face` becomes `result` with `set` applied over the result's default
/// state, charges `wear` durability and grants `give` (one item) to the player.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockToolRule {
    pub item: Identifier,
    pub block: Identifier,
    #[serde(default)]
    pub when: BTreeMap<String, String>,
    pub face: ToolFace,
    pub result: Identifier,
    /// Result properties: a literal value, `"$face"` (the clicked face name) or
    /// `"@name"` (the source block's property `name`).
    #[serde(default)]
    pub set: BTreeMap<String, String>,
    #[serde(default = "default_wear")]
    pub wear: u32,
    #[serde(default)]
    pub give: Option<Identifier>,
}

const fn default_wear() -> u32 {
    1
}

/// Decode and syntactically validate one plugin's rule strings. `owner` is the
/// plugin id the rules' `item` must belong to.
pub fn parse_block_tool_rules(
    owner: &str,
    definitions: &[String],
) -> Result<Vec<BlockToolRule>, String> {
    if definitions.len() > MAX_BLOCK_TOOL_RULES
        || definitions
            .iter()
            .try_fold(0usize, |total, value| total.checked_add(value.len()))
            .is_none_or(|total| total > MAX_BLOCK_TOOL_BYTES)
    {
        return Err(format!(
            "block tools exceed {MAX_BLOCK_TOOL_RULES} rules or {MAX_BLOCK_TOOL_BYTES} bytes"
        ));
    }
    definitions
        .iter()
        .map(|definition| {
            let rule: BlockToolRule =
                serde_json::from_str(definition).map_err(|error| error.to_string())?;
            rule.validate(owner)?;
            Ok(rule)
        })
        .collect()
}

impl BlockToolRule {
    fn validate(&self, owner: &str) -> Result<(), String> {
        if self.item.namespace() != owner {
            return Err(format!(
                "block tool item {} must be owned by {owner}",
                self.item
            ));
        }
        if self.wear == 0 || self.wear > MAX_BLOCK_TOOL_WEAR {
            return Err(format!(
                "block tool wear {} is outside 1..={MAX_BLOCK_TOOL_WEAR}",
                self.wear
            ));
        }
        let property_ok = |name: &str| {
            !name.is_empty()
                && name.len() <= 64
                && name
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        };
        for (name, value) in self.when.iter().chain(&self.set) {
            if !property_ok(name) || value.is_empty() || value.len() > 64 {
                return Err(format!("block tool property {name}={value:?} is invalid"));
            }
        }
        if self
            .set
            .values()
            .any(|value| value.starts_with(COPY_PREFIX) && !property_ok(&value[1..]))
        {
            return Err("block tool copy source is not a property name".to_owned());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rule(extra: &str) -> String {
        format!(
            r#"{{"item":"chisel:chisel","block":"minecraft:stone_bricks","face":"up",
                "result":"minecraft:stone_brick_slab"{extra}}}"#
        )
    }

    #[test]
    fn rule_defaults_wear_to_one_and_requires_owned_item() {
        let rules = parse_block_tool_rules("chisel", &[rule("")]).unwrap();
        assert_eq!(rules[0].wear, 1);
        assert_eq!(rules[0].face, ToolFace::Up);
        assert!(parse_block_tool_rules("other", &[rule("")]).is_err());
    }

    #[test]
    fn rule_refuses_unknown_fields_zero_wear_and_bad_properties() {
        for extra in [
            r#","surprise":1"#,
            r#","wear":0"#,
            r#","wear":32768"#,
            r#","set":{"Type":"top"}"#,
            r#","set":{"type":"@"}"#,
            r#","when":{"half":""}"#,
        ] {
            assert!(
                parse_block_tool_rules("chisel", &[rule(extra)]).is_err(),
                "{extra}"
            );
        }
    }

    #[test]
    fn catalog_bounds_are_enforced() {
        let many = vec![rule(""); MAX_BLOCK_TOOL_RULES + 1];
        assert!(parse_block_tool_rules("chisel", &many).is_err());
    }
}
