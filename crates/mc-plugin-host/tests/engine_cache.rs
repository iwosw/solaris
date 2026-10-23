//! The production engine's persistent compilation cache.
//!
//! Startup recompiles every plugin's component unless Wasmtime's builtin cache
//! is attached: the builtin keys each entry by content hash plus the engine
//! revision and every compile setting, so unchanged components survive a
//! restart while changed bytes or a changed compiler can never reuse stale
//! code. Corrupt entries fall back to recompiling; the builtin never serves
//! bytes it cannot decode.
//!
//! These tests prove the mechanism the production [`engine`] relies on, with
//! the same builtin API and the real fixture, against an isolated cache
//! directory per test: the second engine instance hits the first one's entry,
//! different bytes and different compiler settings compile fresh without
//! aliasing or poisoning each other, and a corrupted directory still compiles.
//! The last test proves the shipped [`engine`] itself caches: it respawns this
//! binary with an isolated cache home, compiles the fixture on two fresh
//! production engines, and asserts the second engine reused the first one's
//! entries instead of recompiling.

mod fixture;

use std::path::{Path, PathBuf};

use fixture::{component_bytes, repo_root};
use mc_plugin_host::{CompiledPlugin, PluginLimits, engine};
use wasmtime::component::Component;
use wasmtime::{Cache, CacheConfig, Config, Engine, OptLevel};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

/// An engine with the builtin cache pointed at `dir`, the same attachment the
/// production engine uses, plus the cache handle so the test can read the
/// builtin's own hit/miss counters.
fn cached_engine(dir: &Path) -> (Engine, Cache) {
    let mut cache_config = CacheConfig::new();
    cache_config.with_directory(dir);
    let cache = Cache::new(cache_config).expect("a temp dir takes a compilation cache");
    let mut config = Config::new();
    config.wasm_component_model(true).cache(Some(cache.clone()));
    let engine = Engine::new(&config).expect("an engine with a cache builds");
    (engine, cache)
}

/// A valid component of the same contract with different bytes: the dummy
/// module for the real WIT, encoded the way a published package is encoded.
fn dummy_component_bytes() -> Vec<u8> {
    let wit: PathBuf = repo_root().join("crates/mc-script/wit");
    let mut resolve = Resolve::new();
    let (package, _) = resolve.push_path(&wit).expect("the WIT parses");
    let world = resolve
        .select_world(&[package], Some("plugin"))
        .expect("the WIT declares the `plugin` world");
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8)
        .expect("the dummy module takes the world's component types");
    ComponentEncoder::default()
        .module(&module)
        .expect("the dummy module carries its component types")
        .validate(true)
        .encode()
        .expect("the dummy module encodes as a component")
}

/// Overwrite every file under `root` with garbage, returning how many files
/// were corrupted. Whatever the cache layout, the next engine must recompile
/// instead of serving or choking on these entries.
fn corrupt_files(root: &Path) -> usize {
    let mut corrupted = 0;
    for entry in std::fs::read_dir(root).expect("the cache directory reads") {
        let path = entry.expect("a cache entry").path();
        if path.is_dir() {
            corrupted += corrupt_files(&path);
        } else if path.is_file() {
            std::fs::write(&path, b"this is not a cached module").expect("a cache file corrupts");
            corrupted += 1;
        }
    }
    corrupted
}

#[test]
fn same_bytes_are_served_from_cache_on_the_next_engine_instance() {
    let dir = tempfile::tempdir().expect("a cache root");
    let bytes = component_bytes();

    let (first, first_cache) = cached_engine(dir.path());
    Component::new(&first, &bytes).expect("the fixture compiles");
    assert!(
        first_cache.cache_misses() >= 1,
        "the first compile populates the cache"
    );
    drop(first);
    drop(first_cache);

    let (second, second_cache) = cached_engine(dir.path());
    Component::new(&second, &bytes).expect("the same bytes compile on a fresh engine");
    assert!(
        second_cache.cache_hits() >= 1,
        "the second engine hits the first engine's cache entry instead of re-running Cranelift"
    );
}

#[test]
fn changed_bytes_and_changed_compiler_settings_do_not_reuse_stale_code() {
    let dir = tempfile::tempdir().expect("a cache root");
    let bytes = component_bytes();
    let other = dummy_component_bytes();
    assert_ne!(bytes, other, "the two components differ");

    let (engine, cache) = cached_engine(dir.path());
    Component::new(&engine, &bytes).expect("the fixture compiles");
    let hits = cache.cache_hits();
    let misses = cache.cache_misses();

    Component::new(&engine, &other).expect("different bytes compile");
    assert_eq!(
        cache.cache_hits(),
        hits,
        "different bytes must not hit the fixture's cache entry"
    );
    assert!(
        cache.cache_misses() > misses,
        "different bytes compile fresh instead of reusing cached code"
    );

    Component::new(&engine, &bytes).expect("the fixture still compiles");
    assert!(
        cache.cache_hits() > hits,
        "the fixture's own entry survived the other component"
    );

    // A changed compiler setting is a different cache key, never a stale hit.
    let mut retuned = Config::new();
    retuned
        .wasm_component_model(true)
        .cranelift_opt_level(OptLevel::None)
        .cache(Some(cache.clone()));
    let retuned_engine = Engine::new(&retuned).expect("a retuned engine builds");
    let hits_before_retune = cache.cache_hits();
    Component::new(&retuned_engine, &bytes).expect("the retuned compile succeeds");
    assert_eq!(
        cache.cache_hits(),
        hits_before_retune,
        "a changed opt level must not hit the old entry"
    );
}

#[test]
fn a_corrupted_cache_falls_back_to_recompiling() {
    let dir = tempfile::tempdir().expect("a cache root");
    let bytes = component_bytes();

    let (engine, cache) = cached_engine(dir.path());
    Component::new(&engine, &bytes).expect("the fixture compiles");
    assert!(
        cache.cache_misses() >= 1,
        "the first compile populates the cache"
    );
    drop(engine);
    drop(cache);

    let corrupted = corrupt_files(dir.path());
    assert!(
        corrupted >= 1,
        "the first compile left cache entries to corrupt"
    );

    let (fresh, _) = cached_engine(dir.path());
    Component::new(&fresh, &bytes)
        .expect("corrupt entries recompile instead of poisoning the engine");
}

/// The `ENGINE_CACHE_CHILD` entry point: compile the fixture once with the
/// shipped [`engine`] and record how many cache entries that produced.
///
/// This runs in a subprocess, never in the test process itself: the cache home
/// moves with the child (via `XDG_CACHE_HOME` when set, or `HOME` otherwise),
/// so the child's engines share one isolated directory without touching the
/// parent process's environment. The count goes to `ENGINE_CACHE_OUT` because
/// the test harness captures a passing test's stdout.
fn engine_cache_child(cache_home: &Path, out: &Path) {
    let before = cache_entries(cache_home);
    let limits = PluginLimits::default();
    let bytes = component_bytes();
    let production = engine(&limits).expect("the production engine builds");
    let compiled = CompiledPlugin::compile(&production, &bytes, &limits, "0.8.0")
        .expect("the fixture compiles on a production engine");
    assert_eq!(compiled.api_version(), "0.8.0");
    std::fs::write(
        out,
        cache_entries(cache_home).saturating_sub(before).to_string(),
    )
    .expect("the child reports its new entries");
}

/// Cache entries below `cache_home`, wherever the platform puts them: the
/// Wasmtime cache lives under its own directory, and the engine revision
/// directory nests deeper still. `*.stats` sidecars are skipped: the cache
/// worker writes them asynchronously, so counting them would race the worker
/// thread, while module files are written synchronously during compilation.
fn cache_entries(cache_home: &Path) -> usize {
    fn walk(root: &Path, count: &mut usize) {
        let Ok(entries) = std::fs::read_dir(root) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, count);
            } else if path.is_file() && path.extension().is_none_or(|ext| ext != "stats") {
                *count += 1;
            }
        }
    }
    let mut count = 0;
    walk(cache_home, &mut count);
    count
}

#[test]
fn the_production_engine_reuses_its_cache_across_restarts() {
    if let (Ok(child_home), Ok(child_out)) = (
        std::env::var("ENGINE_CACHE_CHILD"),
        std::env::var("ENGINE_CACHE_OUT"),
    ) {
        engine_cache_child(Path::new(&child_home), Path::new(&child_out));
        return;
    }
    let home = tempfile::tempdir().expect("an isolated cache home");
    let exe = std::env::current_exe().expect("this test binary");
    let run_child = |extra_env: Option<(&str, &Path)>| -> usize {
        let out = tempfile::NamedTempFile::new().expect("a child report file");
        let mut command = std::process::Command::new(&exe);
        command
            .arg("--exact")
            .arg("the_production_engine_reuses_its_cache_across_restarts")
            .arg("--nocapture")
            .env("ENGINE_CACHE_CHILD", home.path())
            .env("ENGINE_CACHE_OUT", out.path());
        // `XDG_CACHE_HOME` only reroutes the cache on platforms that honour
        // it; `HOME` is the portable half, covering the macOS/Windows cache
        // directory too.
        command.env("XDG_CACHE_HOME", home.path());
        if let Some((key, value)) = extra_env {
            command.env(key, value);
        }
        assert!(
            command.status().expect("the child runs").success(),
            "a production engine child compiles",
        );
        // `NamedTempFile` unlinks on drop, so read before it goes out of scope.
        std::fs::read_to_string(out.path())
            .expect("the child reports its new entries")
            .trim()
            .parse()
            .expect("the child reports a count")
    };
    // A fallback `HOME` for platforms without `XDG_CACHE_HOME`: only consulted
    // when the primary run leaves the cache home empty.
    for fallback in [None, Some(("HOME", home.path()))] {
        let first_new = run_child(fallback);
        if first_new == 0 {
            continue;
        }
        let second_new = run_child(fallback);
        // Without the cache the current production `engine()` is this test's
        // red: no shared directory is consulted, so the second compile writes
        // as many files as the first. With the cache attached the second
        // engine reuses the first one's entries and writes nothing new.
        assert_eq!(
            second_new, 0,
            "the second production engine reuses the first one's cache instead of recompiling"
        );
        return;
    }
    panic!(
        "the production engine wrote no cache entries: cache not enabled, or neither XDG_CACHE_HOME nor HOME reroutes this platform's cache directory"
    );
}
