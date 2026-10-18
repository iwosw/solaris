//! Directory-only v2 deployments retain strict package and real component validation.

#[path = "fixture/mod.rs"]
mod fixture;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use mc_plugin_host::{
    DiscoveryError, HostQueues, NoSessions, PluginLimits, discover_directory, start_deployment,
};
use mc_script::precommit::{
    BuildContext, BuildEdit, HookActor, HookContext, HookFailure, HookFailurePolicy, HookKind,
    HookRegistration,
};

fn package(root: &Path, bytes: &[u8]) {
    let directory = root.join("hello");
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("plugin.toml"),
        "id='hello'\nname='Hello'\nversion='0.1.0'\napi='0.7.0'\nplayer_commands=['hello']\n",
    )
    .unwrap();
    std::fs::write(directory.join("plugin.wasm"), bytes).unwrap();
}

fn hook_package(root: &Path, directory_name: &str, id: &str, hooks: &str, bytes: &[u8]) {
    let directory = root.join(directory_name);
    std::fs::create_dir_all(&directory).unwrap();
    std::fs::write(
        directory.join("plugin.toml"),
        format!("id={id:?}\nname={id:?}\nversion='0.1.0'\napi='0.7.0'\nhooks=[{hooks}]\n"),
    )
    .unwrap();
    std::fs::write(directory.join("plugin.wasm"), bytes).unwrap();
}

#[test]
fn directory_only_startup_loads_a_real_wit_component_without_an_expected_id_list() {
    let directory = tempfile::tempdir().unwrap();
    package(directory.path(), &fixture::component_bytes());
    let limits = PluginLimits::default();
    let deployment =
        discover_directory(directory.path().to_path_buf(), &limits, &[HookKind::Build]).unwrap();
    assert_eq!(deployment.packages().len(), 1);
    assert!(deployment.skipped().is_empty());
    assert!(
        deployment.packages()[0]
            .precommit_registrations()
            .is_empty()
    );
    let host = start_deployment(
        deployment.into_packages(),
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .unwrap();
    assert!(host.boundary().plugin_is_active("hello"));
    assert!(!host.boundary().has_precommit_hooks(HookKind::Build));
    assert!(!host.boundary().has_precommit_hooks(HookKind::Damage));
    assert!(mc_plugin_host::wasm_invocations() > 0);
    host.stop();
}

#[test]
fn directory_only_deployment_refuses_every_invalid_entry_and_invalid_wasm() {
    let limits = PluginLimits::default();
    let directory = tempfile::tempdir().unwrap();
    package(directory.path(), b"not a Wasm component");
    let deployment = discover_directory(directory.path().to_path_buf(), &limits, &[]).unwrap();
    assert!(
        start_deployment(
            deployment.into_packages(),
            limits,
            HostQueues::default(),
            Arc::new(NoSessions),
        )
        .is_err()
    );
    std::fs::write(directory.path().join("stray.txt"), b"not a package").unwrap();
    assert!(discover_directory(directory.path().to_path_buf(), &limits, &[]).is_err());

    let invalid = tempfile::tempdir().unwrap();
    std::fs::create_dir(invalid.path().join("broken")).unwrap();
    std::fs::write(invalid.path().join("broken/plugin.toml"), "id='broken'\n").unwrap();
    assert!(discover_directory(invalid.path().to_path_buf(), &limits, &[]).is_err());
}

#[test]
fn directory_build_roster_is_complete_deny_on_failure_and_sorted_by_plugin_id() {
    for (alpha_directory, zulu_directory) in
        [("z-package", "a-package"), ("a-package", "z-package")]
    {
        let directory = tempfile::tempdir().unwrap();
        hook_package(
            directory.path(),
            zulu_directory,
            "zulu",
            "'before-build'",
            b"component bytes are not compiled by discovery",
        );
        hook_package(
            directory.path(),
            alpha_directory,
            "alpha",
            "'before-build'",
            b"component bytes are not compiled by discovery",
        );
        let deployment = discover_directory(
            directory.path().to_path_buf(),
            &PluginLimits::default(),
            &[HookKind::Build],
        )
        .unwrap();
        let roster = deployment
            .packages()
            .iter()
            .flat_map(|package| package.precommit_registrations().iter().cloned())
            .collect::<Vec<_>>();
        assert_eq!(
            roster,
            vec![
                HookRegistration::new("alpha", HookKind::Build, 0, HookFailurePolicy::Deny),
                HookRegistration::new("zulu", HookKind::Build, 0, HookFailurePolicy::Deny),
            ]
        );
        assert!(deployment.skipped().is_empty());
    }
}

#[test]
fn directory_refuses_unsupported_hooks_even_when_the_same_package_declares_build() {
    for hooks in ["'before-damage'", "'before-build', 'before-damage'"] {
        let directory = tempfile::tempdir().unwrap();
        hook_package(
            directory.path(),
            "hook-owner",
            "hook-owner",
            hooks,
            b"not a Wasm component",
        );
        let error = discover_directory(
            directory.path().to_path_buf(),
            &PluginLimits::default(),
            &[HookKind::Build],
        )
        .expect_err("unsupported declarations must fail before component compilation");
        assert!(
            matches!(
                error,
                DiscoveryError::UnsupportedHook { ref id, kind: HookKind::Damage }
                    if id == "hook-owner"
            ),
            "{error}"
        );
    }
}

#[test]
fn directory_with_no_supported_hooks_refuses_build_declarations() {
    let directory = tempfile::tempdir().unwrap();
    hook_package(
        directory.path(),
        "hook-owner",
        "hook-owner",
        "'before-build'",
        b"not a Wasm component",
    );
    let error = discover_directory(
        directory.path().to_path_buf(),
        &PluginLimits::default(),
        &[],
    )
    .expect_err("discovery must not silently ignore declarations");
    assert!(
        matches!(
            error,
            DiscoveryError::UnsupportedHook { ref id, kind: HookKind::Build }
                if id == "hook-owner"
        ),
        "{error}"
    );
}

#[tokio::test]
async fn directory_build_declaration_activates_the_real_host_hook() {
    let directory = tempfile::tempdir().unwrap();
    hook_package(
        directory.path(),
        "hook-owner",
        "hook-owner",
        "'before-build'",
        &fixture::component_bytes(),
    );
    std::fs::write(
        directory.path().join("hook-owner/config.toml"),
        "mode = 'precommit'\nbuild = 'cancel'\n",
    )
    .unwrap();
    let limits = PluginLimits::default();
    let deployment =
        discover_directory(directory.path().to_path_buf(), &limits, &[HookKind::Build]).unwrap();
    let host = start_deployment(
        deployment.into_packages(),
        limits,
        HostQueues::default(),
        Arc::new(NoSessions),
    )
    .unwrap();
    assert!(host.boundary().has_precommit_hooks(HookKind::Build));
    assert!(!host.boundary().has_precommit_hooks(HookKind::Damage));
    let question = HookContext::Build(
        BuildContext::try_new(
            HookActor::Environment,
            "minecraft:overworld",
            vec![BuildEdit::new(1, 64, -2, 0, 4)],
        )
        .unwrap(),
    );
    let pending = host.boundary().begin_precommit(question).unwrap();
    let mut approval = tokio::time::timeout(Duration::from_secs(10), pending.resolve())
        .await
        .expect("real component answers the build question")
        .expect("chain returns its cancellation");
    assert_eq!(approval.consume(), Err(HookFailure::Cancelled));
    host.stop();
}
