//! Path-scoped file loading shared by registry-owned vanilla catalogs.
//!
//! Startup passes its selected content-cache root to each catalog loader. No
//! process-global state, discovery, or compile-time vanilla bytes live here.

use std::path::Path;

/// Read a vanilla JSON source, reporting the exact missing or unreadable path.
/// Callers parse it into their immutable startup-owned table before serving.
pub(crate) fn read_json(root: &Path, relative: &str) -> Result<String, String> {
    let path = root.join(relative);
    crate::sidecar::read_string(&path).map_err(|error| {
        format!(
            "loading required vanilla resource {}: {error}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_resource_reports_exact_file() {
        let directory = tempfile::tempdir().unwrap();
        let relative = "data/minecraft/loot_table/entities/cow.json";
        let error = read_json(directory.path(), relative).unwrap_err();
        assert!(
            error.contains(&directory.path().join(relative).display().to_string()),
            "{error}"
        );
    }

    #[test]
    fn independent_roots_do_not_share_sources() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let relative = "table.json";
        std::fs::write(first.path().join(relative), "{}").unwrap();
        std::fs::write(second.path().join(relative), "[]").unwrap();
        assert_eq!(read_json(first.path(), relative).unwrap(), "{}");
        assert_eq!(read_json(second.path(), relative).unwrap(), "[]");
        assert_eq!(read_json(first.path(), relative).unwrap(), "{}");
    }
}
