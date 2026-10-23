//! The package contract: what a component package directory may say, and what is
//! refused before any guest code exists.

use mc_plugin_host::{PackageError, PluginLimits, load_package};
use mc_script::{ClientContentKind, ClientLoader, ClientPermission};

/// One package directory with `plugin.toml` and an artifact of `artifact` bytes.
fn package(manifest: &str, artifact: &[u8]) -> tempfile::TempDir {
    package_with(manifest, artifact, &[])
}

/// As [`package`], additionally writing every extra file under the package root.
fn package_with(manifest: &str, artifact: &[u8], files: &[(&str, &[u8])]) -> tempfile::TempDir {
    let root = tempfile::tempdir().expect("temp package root");
    std::fs::write(root.path().join("plugin.toml"), manifest).expect("manifest");
    if !artifact.is_empty() {
        std::fs::write(root.path().join("plugin.wasm"), artifact).expect("artifact");
    }
    for (path, bytes) in files {
        let file = root.path().join(path);
        std::fs::create_dir_all(file.parent().expect("extra file has a parent"))
            .expect("extra file directory");
        std::fs::write(file, bytes).expect("extra file");
    }
    root
}

fn manifest(api: &str, extra: &str) -> String {
    format!("id = \"hello\"\nname = \"Hello\"\nversion = \"0.1.0\"\napi = \"{api}\"\n{extra}")
}

/// One bundle every loader, content kind and permission of the contract covers,
/// with the artifact bytes and hash the fixture writes to disk.
const CLIENT_BUNDLE: &str = r#"
[client]
schema = 2

[[client.bundles]]
id = "rich-content"
version = "1.2.3"
artifact = "client/rich-content.zip"
sha256 = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
size_bytes = 1
loaders = ["fabric", "neoforge", "forge", "native"]
content = ["blocks", "items", "views", "view_actions", "assets"]
permissions = [
  "register_blocks",
  "register_items",
  "present_views",
  "send_view_actions",
  "load_assets",
]
"#;

/// The one byte `CLIENT_BUNDLE` hashes: the artifact a valid bundle names.
const CLIENT_ARTIFACT: &[u8] = b"x";
/// `sha256(CLIENT_ARTIFACT)`, which is what the bundle above declares.
const CLIENT_ARTIFACT_SHA256: &str =
    "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881";
/// The artifact file `CLIENT_BUNDLE` names.
const CLIENT_ARTIFACT_FILE: &[(&str, &[u8])] = &[("client/rich-content.zip", CLIENT_ARTIFACT)];
/// The artifact file the refusal cases that name `client/assets.zip` write.
const ASSET_ARTIFACT_FILE: &[(&str, &[u8])] = &[("client/assets.zip", CLIENT_ARTIFACT)];
/// The artifact file the refusal case that names `client/screen.zip` writes.
const SCREEN_ARTIFACT_FILE: &[(&str, &[u8])] = &[("client/screen.zip", CLIENT_ARTIFACT)];
/// No artifact at all, for the case that declares one nothing ships.
const NO_FILES: &[(&str, &[u8])] = &[];

#[test]
fn a_component_package_loads_with_its_manifest_contract() {
    let root = package(
        &manifest(
            "0.8.0",
            "events = [\"player.joined\"]\ncapabilities = [\"player_inventory\"]\nplayer_commands = [\"hello\"]\n",
        ),
        b"component-bytes",
    );
    let package = load_package(root.path(), &PluginLimits::default()).expect("package loads");
    assert_eq!(package.manifest().plugin_id(), "hello");
    assert_eq!(package.artifact(), b"component-bytes");
    assert_eq!(package.root(), root.path());
    assert!(
        package
            .manifest()
            .player_command_roots()
            .contains(&"hello".to_owned()),
        "the declared command root reaches the manifest contract"
    );
}

#[test]
fn the_component_contract_refuses_an_obsolete_api_version() {
    let root = package(&manifest("0.6.0", ""), b"component-bytes");
    let error = load_package(root.path(), &PluginLimits::default())
        .expect_err("the obsolete API version is not the component contract");
    assert!(
        matches!(error, PackageError::Manifest { .. }),
        "the refusal names the manifest: {error}"
    );
}

#[test]
fn an_unknown_capability_is_refused_rather_than_ignored() {
    let root = package(
        &manifest("0.8.0", "capabilities = [\"chat.send\"]\n"),
        b"component-bytes",
    );
    let error = load_package(root.path(), &PluginLimits::default())
        .expect_err("an unknown capability must fail the package");
    assert!(
        format!("{error}").contains("capability"),
        "the refusal names the capability: {error}"
    );
}

#[test]
fn an_entry_that_leaves_the_package_is_refused() {
    for entry in ["../outside.wasm", "/etc/passwd", ""] {
        let root = package(
            &manifest("0.8.0", &format!("entry = \"{entry}\"\n")),
            b"component-bytes",
        );
        let error = load_package(root.path(), &PluginLimits::default())
            .expect_err("an entry outside the package must be refused");
        assert!(
            matches!(error, PackageError::Escape { .. } | PackageError::Io { .. }),
            "entry {entry:?} was refused as {error}"
        );
    }
}

#[test]
fn an_artifact_past_the_bound_is_refused_before_compilation() {
    let root = package(&manifest("0.8.0", ""), &vec![0_u8; 4096]);
    let limits = PluginLimits {
        artifact_bytes: 1024,
        ..PluginLimits::default()
    };
    let error = load_package(root.path(), &limits).expect_err("the artifact bound applies");
    assert!(
        matches!(error, PackageError::ArtifactTooLarge { .. }),
        "the refusal is about size, not compilation: {error}"
    );
}

#[test]
fn a_client_bundle_reaches_the_loader_as_validated_bytes() {
    let root = package_with(
        &manifest("0.8.0", CLIENT_BUNDLE),
        b"component-bytes",
        CLIENT_ARTIFACT_FILE,
    );
    let loaded = load_package(root.path(), &PluginLimits::default()).expect("package loads");
    let bundles = loaded.client_bundles();

    assert_eq!(bundles.len(), 1, "the declared bundle is the deployed one");
    let bundle = &bundles[0];
    assert_eq!(bundle.owner_plugin_id(), "hello");
    assert_eq!(bundle.id(), "rich-content");
    assert_eq!(bundle.version(), "1.2.3");
    assert_eq!(bundle.sha256(), CLIENT_ARTIFACT_SHA256);
    assert_eq!(bundle.size_bytes(), 1);
    assert_eq!(
        bundle.loaders(),
        &[
            ClientLoader::Fabric,
            ClientLoader::NeoForge,
            ClientLoader::Forge,
            ClientLoader::Native,
        ]
    );
    assert_eq!(
        bundle.content(),
        &[
            ClientContentKind::Blocks,
            ClientContentKind::Items,
            ClientContentKind::Views,
            ClientContentKind::ViewActions,
            ClientContentKind::Assets,
        ]
    );
    assert_eq!(
        bundle.permissions(),
        &[
            ClientPermission::RegisterBlocks,
            ClientPermission::RegisterItems,
            ClientPermission::PresentViews,
            ClientPermission::SendViewActions,
            ClientPermission::LoadAssets,
        ]
    );
    assert_eq!(
        bundle.cache_key(),
        format!("hello:rich-content/1.2.3/{CLIENT_ARTIFACT_SHA256}")
    );
    assert_eq!(
        bundle.artifact_path(),
        std::fs::canonicalize(root.path().join("client/rich-content.zip")).unwrap()
    );
    assert_eq!(bundle.artifact_bytes(), CLIENT_ARTIFACT);

    // What a session is served is the bytes the host hashed, not whatever the
    // file holds later: a deployment that swaps its artifact mid-run cannot
    // change what an accepted bundle delivers, and the next load refuses the swap.
    std::fs::write(root.path().join("client/rich-content.zip"), b"y").unwrap();
    assert_eq!(bundle.artifact_bytes(), CLIENT_ARTIFACT);
    let swapped = load_package(root.path(), &PluginLimits::default())
        .expect_err("the swapped artifact no longer matches the manifest");
    assert!(
        swapped.to_string().contains("SHA-256 does not match"),
        "the next load hashes the file again: {swapped}"
    );
}

#[test]
fn a_client_bundle_that_does_not_hold_up_is_refused() {
    let cases = [
        (
            "schema",
            r#"[client]
schema = 3

[[client.bundles]]
id = "assets"
version = "1"
artifact = "client/assets.zip"
sha256 = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
size_bytes = 1
loaders = ["fabric"]
content = ["assets"]
permissions = ["load_assets"]
"#,
            ASSET_ARTIFACT_FILE,
            "client manifest schema must be 2",
        ),
        (
            "hash",
            r#"[client]
schema = 2

[[client.bundles]]
id = "assets"
version = "1"
artifact = "client/assets.zip"
sha256 = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
size_bytes = 1
loaders = ["fabric"]
content = ["assets"]
permissions = ["load_assets"]
"#,
            ASSET_ARTIFACT_FILE,
            "SHA-256 does not match",
        ),
        (
            "size",
            r#"[client]
schema = 2

[[client.bundles]]
id = "assets"
version = "1"
artifact = "client/assets.zip"
sha256 = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
size_bytes = 2
loaders = ["fabric"]
content = ["assets"]
permissions = ["load_assets"]
"#,
            ASSET_ARTIFACT_FILE,
            "manifest declares 2",
        ),
        (
            "path",
            r#"[client]
schema = 2

[[client.bundles]]
id = "assets"
version = "1"
artifact = "../assets.zip"
sha256 = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
size_bytes = 1
loaders = ["fabric"]
content = ["assets"]
permissions = ["load_assets"]
"#,
            ASSET_ARTIFACT_FILE,
            "client artifact path",
        ),
        (
            "permission",
            r#"[client]
schema = 2

[[client.bundles]]
id = "screen"
version = "1"
artifact = "client/screen.zip"
sha256 = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
size_bytes = 1
loaders = ["fabric"]
content = ["views"]
permissions = ["load_assets"]
"#,
            SCREEN_ARTIFACT_FILE,
            "requires permission",
        ),
        (
            "absent-artifact",
            r#"[client]
schema = 2

[[client.bundles]]
id = "assets"
version = "1"
artifact = "client/assets.zip"
sha256 = "2d711642b726b04401627ca9fbac32f5c8530fb1903cc4db02258717921a4881"
size_bytes = 1
loaders = ["fabric"]
content = ["assets"]
permissions = ["load_assets"]
"#,
            NO_FILES,
            "opening client artifact",
        ),
    ];

    for (case, client, files, expected) in cases {
        let root = package_with(&manifest("0.8.0", client), b"component-bytes", files);
        let error = load_package(root.path(), &PluginLimits::default())
            .expect_err("a bundle that cannot be delivered must fail the package");
        assert!(
            matches!(error, PackageError::ClientBundle { .. }),
            "{case}: the refusal is about the declared bundle, saw {error}"
        );
        assert!(
            error.to_string().contains(expected),
            "{case}: expected {expected:?} in {error}"
        );
    }
}

/// A bundle names a path, never a place the host reads from: an artifact that
/// resolves outside the package is refused even when its bytes and hash are
/// exactly the ones the manifest declares.
#[cfg(unix)]
#[test]
fn a_client_artifact_that_escapes_the_package_is_refused() {
    let root = package(&manifest("0.8.0", CLIENT_BUNDLE), b"component-bytes");
    let outside = tempfile::tempdir().expect("outside directory");
    std::fs::write(outside.path().join("rich-content.zip"), CLIENT_ARTIFACT)
        .expect("outside artifact");
    std::fs::create_dir_all(root.path().join("client")).expect("client directory");
    std::os::unix::fs::symlink(
        outside.path().join("rich-content.zip"),
        root.path().join("client/rich-content.zip"),
    )
    .expect("escaping symlink");

    let error = load_package(root.path(), &PluginLimits::default())
        .expect_err("an artifact outside the package must be refused");
    assert!(
        matches!(error, PackageError::ClientBundle { .. }),
        "the refusal is about the declared bundle: {error}"
    );
    assert!(
        error.to_string().contains("escapes the plugin directory"),
        "the refusal names the escape: {error}"
    );
}

#[test]
fn a_feature_capability_must_be_required_and_not_merely_declared() {
    let declared = "capabilities = [\"world_structures\"]\n";
    let root = package(&manifest("0.8.0", declared), b"component-bytes");
    let error = load_package(root.path(), &PluginLimits::default())
        .expect_err("a feature capability without its feature must be refused");
    assert!(matches!(error, PackageError::Manifest { .. }));

    let root = package(
        &manifest(
            "0.8.0",
            "capabilities = [\"world_structures\"]\nrequired_features = [\"world_structures\"]\n",
        ),
        b"component-bytes",
    );
    let loaded = load_package(root.path(), &PluginLimits::default()).expect("package loads");
    assert_eq!(
        loaded.required_features().to_vec(),
        vec!["world_structures".to_owned()]
    );

    let root = package(
        &manifest("0.8.0", "required_features = [\"unknown_feature\"]\n"),
        b"component-bytes",
    );
    let error = load_package(root.path(), &PluginLimits::default())
        .expect_err("a feature core does not know must be refused, not ignored");
    assert!(matches!(error, PackageError::Manifest { .. }));
}
