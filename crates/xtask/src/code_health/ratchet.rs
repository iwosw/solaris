//! Anti-pattern trend report for `mc-core` production code (diagnostic only).
//!
//! Text counts cannot tell comments or legitimate NBT/protocol ids from
//! defects, so they never fail code-health; they show direction against the
//! recorded baseline until semantic rules replace them. Crate totals (not
//! per-file) keep the numbers stable across pure file splits.

use std::fs;
use std::path::Path;

/// (pattern, baseline at integration c09e1391, why).
const CEILINGS: &[(&str, usize, &str)] = &[
    (
        ".path() ==",
        88,
        "stringly-typed block/item identity; resolve a typed kind once at load",
    ),
    (
        "\"minecraft:",
        2441,
        "inline vanilla id literal; use a typed kind/tag resolved at load",
    ),
    (
        "panic!(",
        17,
        "crash path; persisted/external data must return an error",
    ),
    (
        "merge_block_writes.push(",
        35,
        "hand-rolled block write; use TickState::write_block",
    ),
];

fn production_sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            production_sources(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs")
            && !path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.ends_with("_tests.rs") || name == "tests.rs")
        {
            out.push(path);
        }
    }
}

/// Counts outside inline `#[cfg(test)]` modules: everything after the first
/// `#[cfg(test)]\nmod ... {` line of a file is treated as test code.
fn count(source: &str, pattern: &str) -> usize {
    let production = source
        .find("#[cfg(test)]\nmod tests {")
        .map_or(source, |end| &source[..end]);
    production.matches(pattern).count()
}

pub(super) fn report(root: &Path) {
    let src = root.join("crates/mc-core/src");
    let mut files = Vec::new();
    production_sources(&src, &mut files);
    let sources: Vec<String> = files
        .iter()
        .filter_map(|path| fs::read_to_string(path).ok())
        .collect();
    println!("anti-pattern trend (diagnostic, mc-core production):");
    for &(pattern, baseline, why) in CEILINGS {
        let total: usize = sources.iter().map(|source| count(source, pattern)).sum();
        println!("  `{pattern}`: {total} (baseline {baseline}) - {why}");
    }
    println!();
}
