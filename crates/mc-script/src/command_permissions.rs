//! Cross-plugin command permission grants.
//!
//! A manifest may require a permission node for one of its command roots
//! (`command_permissions`), and exactly one deployed package may declare itself
//! the permission provider (`permission_provider`). The provider is the only
//! package the host hands [`ScriptCommandPermissions`] to, and it publishes the
//! whole grant table with one bounded, replace-only call.
//!
//! The table is consulted by command admission on the tick thread, so publication
//! swaps one immutable snapshot: admission never waits for a guest and a failed
//! publication never leaves half a table behind.
//!
//! Entry shape is `"<identity> <node>"`, where `identity` is a lower-case dashed
//! UUID, a compact 32-hex UUID, or `*`. Identity `*` is the *default* row: it
//! applies to a player only when the table holds no explicit row for that player,
//! which is what makes a group assignment override the default group instead of
//! adding to it. A node of `*` grants every gated root.
//!
//! A deployment without a permission provider does not manage permissions: gated
//! roots then behave exactly as they did before the declaration, so a package
//! that ships nodes is not silently disabled on a server that never opted into a
//! permission authority.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use arc_swap::ArcSwap;

/// Entries one publication may carry.
pub const MAX_COMMAND_PERMISSION_ENTRIES: usize = 8192;
/// Bytes one `"<identity> <node>"` entry may carry.
pub const MAX_COMMAND_PERMISSION_ENTRY_BYTES: usize = 192;
/// Bytes one permission node may carry.
pub const MAX_COMMAND_PERMISSION_NODE_BYTES: usize = 128;

/// Why a publication was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptCommandPermissionError {
    /// No deployed package is the permission provider.
    NoProvider,
    /// More entries than [`MAX_COMMAND_PERMISSION_ENTRIES`].
    TooManyEntries { requested: usize },
    /// One entry was blank, over-long, or not `"<identity> <node>"`.
    InvalidEntry { entry: String },
}

impl std::fmt::Display for ScriptCommandPermissionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoProvider => formatter.write_str("no permission provider is deployed"),
            Self::TooManyEntries { requested } => write!(
                formatter,
                "{requested} permission entries exceed the bound of {MAX_COMMAND_PERMISSION_ENTRIES}"
            ),
            Self::InvalidEntry { entry } => {
                write!(
                    formatter,
                    "permission entry {entry:?} is not \"<identity> <node>\""
                )
            }
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct PermissionTable {
    provider: Option<String>,
    grants: BTreeMap<String, BTreeSet<String>>,
}

/// The authority's published command-permission table.
///
/// Cloned through [`Arc`]; every clone sees the same published snapshot.
#[derive(Debug, Default)]
pub struct ScriptCommandPermissions {
    table: ArcSwap<PermissionTable>,
}

impl ScriptCommandPermissions {
    /// Record which package is the permission provider, or clear the authority.
    ///
    /// Unregistering the provider forgets its grants: the deployment no longer
    /// has a permission authority, so gated roots return to unmanaged behaviour.
    pub(crate) fn set_provider(&self, provider: Option<&str>) {
        let mut table = (**self.table.load()).clone();
        if table.provider.as_deref() == provider {
            return;
        }
        table.provider = provider.map(str::to_owned);
        table.grants.clear();
        self.table.store(Arc::new(table));
    }

    /// Whether a package currently answers for command permissions.
    #[must_use]
    pub fn provider_deployed(&self) -> bool {
        self.table.load().provider.is_some()
    }

    /// The package id that answers for command permissions, when one is deployed.
    #[must_use]
    pub fn provider(&self) -> Option<String> {
        self.table.load().provider.clone()
    }

    /// Replace the whole grant table with one validated publication.
    ///
    /// A refused publication leaves the previous table exactly as it was.
    pub fn publish(&self, entries: &[String]) -> Result<(), ScriptCommandPermissionError> {
        if !self.provider_deployed() {
            return Err(ScriptCommandPermissionError::NoProvider);
        }
        if entries.len() > MAX_COMMAND_PERMISSION_ENTRIES {
            return Err(ScriptCommandPermissionError::TooManyEntries {
                requested: entries.len(),
            });
        }
        let mut grants: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for entry in entries {
            let (identity, node) =
                parse_entry(entry).ok_or_else(|| ScriptCommandPermissionError::InvalidEntry {
                    entry: entry.clone(),
                })?;
            grants.entry(identity).or_default().insert(node);
        }
        let table = PermissionTable {
            provider: self.table.load().provider.clone(),
            grants,
        };
        self.table.store(Arc::new(table));
        Ok(())
    }

    /// Whether `uuid` may run a root that requires `node`.
    ///
    /// Identities are compared in one canonical form: a provider may publish a
    /// dashed or compact UUID, and the native context writes the dashed form, so
    /// both sides normalize to compact lower-case hex.
    ///
    /// Unmanaged deployments admit everything: only a deployed provider turns a
    /// declared node into a requirement.
    #[must_use]
    pub fn allows(&self, uuid: &str, node: &str) -> bool {
        let table = self.table.load();
        if table.provider.is_none() {
            return true;
        }
        let Some(identity) = normalize_identity(uuid) else {
            return false;
        };
        let nodes = table
            .grants
            .get(&identity)
            .or_else(|| table.grants.get("*"));
        nodes.is_some_and(|nodes| nodes.contains(node) || nodes.contains("*"))
    }
}

/// Parse `"<identity> <node>"`, returning the canonical identity and node.
fn parse_entry(entry: &str) -> Option<(String, String)> {
    if entry.len() > MAX_COMMAND_PERMISSION_ENTRY_BYTES {
        return None;
    }
    let (identity, node) = entry.split_once(' ')?;
    if !valid_node(node) {
        return None;
    }
    Some((normalize_identity(identity)?, node.to_owned()))
}

/// One canonical identity: `*` or compact lower-case UUID hex.
fn normalize_identity(identity: &str) -> Option<String> {
    if identity == "*" {
        return Some("*".to_owned());
    }
    if !valid_identity(identity) {
        return None;
    }
    Some(
        identity
            .bytes()
            .filter(|byte| *byte != b'-')
            .map(char::from)
            .collect(),
    )
}

fn valid_identity(identity: &str) -> bool {
    if identity == "*" {
        return true;
    }
    // A dashed UUID keeps its dashes exactly where vanilla writes them; a
    // compact identity is 32 hex digits and nothing else.
    let compact = match identity.len() {
        36 => {
            let mut compact = String::with_capacity(32);
            for (index, byte) in identity.bytes().enumerate() {
                if matches!(index, 8 | 13 | 18 | 23) {
                    if byte != b'-' {
                        return false;
                    }
                } else if byte == b'-' {
                    return false;
                } else {
                    compact.push(char::from(byte));
                }
            }
            compact
        }
        32 => identity.to_owned(),
        _ => return false,
    };
    compact
        .bytes()
        .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn valid_node(node: &str) -> bool {
    !node.is_empty()
        && node.len() <= MAX_COMMAND_PERMISSION_NODE_BYTES
        && node.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'.' | b'_' | b'-' | b'*' | b'@' | b':' | b'/')
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALEX: &str = "069a79f4-44e9-4726-a5be-fca90e38aaf5";

    fn managed() -> ScriptCommandPermissions {
        let permissions = ScriptCommandPermissions::default();
        permissions.set_provider(Some("solaris-permissions"));
        permissions
    }

    #[test]
    fn unmanaged_deployments_do_not_gate_declared_nodes() {
        let permissions = ScriptCommandPermissions::default();
        assert!(!permissions.provider_deployed());
        assert!(permissions.allows(ALEX, "solaris.essentials.home"));
        assert_eq!(
            permissions.publish(&[format!("{ALEX} solaris.essentials.home")]),
            Err(ScriptCommandPermissionError::NoProvider)
        );
    }

    #[test]
    fn publication_replaces_the_table_and_refusals_keep_the_previous_one() {
        let permissions = managed();
        permissions
            .publish(&[format!("{ALEX} solaris.essentials.home")])
            .unwrap();
        assert!(permissions.allows(ALEX, "solaris.essentials.home"));
        assert!(!permissions.allows(ALEX, "solaris.towns.claim"));
        assert_eq!(
            permissions.publish(&["Alex solaris.towns.claim".to_owned()]),
            Err(ScriptCommandPermissionError::InvalidEntry {
                entry: "Alex solaris.towns.claim".to_owned()
            })
        );
        assert!(permissions.allows(ALEX, "solaris.essentials.home"));
        permissions.publish(&[]).unwrap();
        assert!(!permissions.allows(ALEX, "solaris.essentials.home"));
    }

    #[test]
    fn default_row_applies_only_without_an_explicit_player_row() {
        let permissions = managed();
        permissions
            .publish(&[
                "* solaris.essentials.use".to_owned(),
                format!("{ALEX} solaris.towns.claim"),
            ])
            .unwrap();
        assert!(!permissions.allows(ALEX, "solaris.essentials.use"));
        assert!(permissions.allows(ALEX, "solaris.towns.claim"));
        let stranger = "00000000-0000-0000-0000-000000000001";
        assert!(permissions.allows(stranger, "solaris.essentials.use"));
        assert!(!permissions.allows(stranger, "solaris.towns.claim"));
    }

    #[test]
    fn dashed_and_compact_identities_are_the_same_player() {
        let permissions = managed();
        // A provider (and the permissions plugin's storage) publishes compact
        // hex while the native context asks with the dashed UUID.
        permissions
            .publish(&["069a79f444e94726a5befca90e38aaf5 solaris.essentials.use".to_owned()])
            .unwrap();
        assert!(permissions.allows(ALEX, "solaris.essentials.use"));
        assert!(!permissions.allows(ALEX, "solaris.essentials.manage"));
        // ...and a dashed publication answers the compact query too.
        permissions
            .publish(&[format!("{ALEX} solaris.essentials.manage")])
            .unwrap();
        assert!(permissions.allows(
            "069a79f444e94726a5befca90e38aaf5",
            "solaris.essentials.manage"
        ));
    }

    #[test]
    fn compact_identity_and_star_node_grant_everything() {
        let permissions = managed();
        permissions
            .publish(&["069a79f444e94726a5befca90e38aaf5 *".to_owned()])
            .unwrap();
        assert!(permissions.allows(ALEX, "anything.at.all"));
    }

    #[test]
    fn provider_removal_forgets_grants() {
        let permissions = managed();
        permissions
            .publish(&[format!("{ALEX} solaris.essentials.home")])
            .unwrap();
        permissions.set_provider(None);
        assert!(!permissions.provider_deployed());
        assert!(permissions.allows(ALEX, "solaris.essentials.home"));
        permissions.set_provider(Some("solaris-permissions"));
        assert!(!permissions.allows(ALEX, "solaris.essentials.home"));
    }
}
