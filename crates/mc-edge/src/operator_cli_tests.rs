use super::*;

#[test]
fn content_import_accepts_licensed_jar_or_download_and_pinned_default() {
    for source in [vec![], vec!["--download"], vec!["--from", "server.jar"]] {
        let mut args = vec!["solaris-v2", "content", "import", "--cache", "cache"];
        args.extend(source);
        let cli = Cli::try_parse_from(args).unwrap();
        let Some(Command::Content {
            command: ContentCommand::Import { version, cache, .. },
        }) = cli.command
        else {
            panic!("content import")
        };
        assert_eq!(version, mc_protocol::TARGET_RELEASE);
        assert_eq!(cache.unwrap(), PathBuf::from("cache"));
    }
    assert!(
        Cli::try_parse_from([
            "solaris-v2",
            "content",
            "import",
            "--from",
            "server.jar",
            "--download"
        ])
        .is_err()
    );
    assert!(Cli::try_parse_from(["solaris-v2", "content", "verify", "--download"]).is_err());
}

#[test]
fn offline_commands_require_exact_paths_and_global_config() {
    for name in ["import-anvil", "export-anvil"] {
        let cli = Cli::try_parse_from([
            "solaris-v2",
            "world",
            name,
            "--source",
            "source",
            "--destination",
            "destination",
            "--receipt",
            "receipt.json",
            "--config",
            "conversion.toml",
        ])
        .unwrap();
        assert_eq!(cli.config, PathBuf::from("conversion.toml"));
        let Some(Command::World { command }) = cli.command else {
            panic!("world")
        };
        let paths = match command {
            WorldCommand::ImportAnvil(paths) | WorldCommand::ExportAnvil(paths) => paths,
            WorldCommand::Census(_)
            | WorldCommand::Probe(_)
            | WorldCommand::UpgradeWorldgen
            | WorldCommand::Pregenerate { .. }
            | WorldCommand::Prune { .. }
            | WorldCommand::Compact => unreachable!("conversion names only"),
        };
        assert_eq!(paths.source, PathBuf::from("source"));
        assert_eq!(paths.destination, PathBuf::from("destination"));
        assert_eq!(paths.receipt, PathBuf::from("receipt.json"));
        assert!(
            Cli::try_parse_from([
                "solaris-v2",
                "world",
                name,
                "--source",
                "source",
                "--destination",
                "destination"
            ])
            .is_err()
        );
    }
    for command in ["backup", "restore"] {
        assert!(Cli::try_parse_from(["solaris-v2", command, "--world-dir", "world"]).is_err());
        let option = if command == "backup" {
            "--out"
        } else {
            "--from"
        };
        assert!(
            Cli::try_parse_from([
                "solaris-v2",
                command,
                option,
                "backup",
                "--world-dir",
                "world"
            ])
            .is_ok()
        );
    }
}

#[test]
fn world_census_takes_source_and_receipt_only() {
    let cli = Cli::try_parse_from([
        "solaris-v2",
        "--config",
        "census.toml",
        "world",
        "census",
        "--source",
        "world",
        "--receipt",
        "census.json",
    ])
    .unwrap();
    assert_eq!(cli.config, PathBuf::from("census.toml"));
    let Some(Command::World {
        command: WorldCommand::Census(paths),
    }) = cli.command
    else {
        panic!("world census")
    };
    assert_eq!(paths.source, PathBuf::from("world"));
    assert_eq!(paths.receipt, PathBuf::from("census.json"));
    for missing in [
        vec!["--source", "world"],
        vec!["--receipt", "census.json"],
        vec![
            "--source",
            "world",
            "--receipt",
            "census.json",
            "--destination",
            "out",
        ],
    ] {
        let mut args = vec!["solaris-v2", "world", "census"];
        args.extend(missing);
        assert!(Cli::try_parse_from(args).is_err());
    }
}

#[test]
fn world_probe_takes_source_receipt_and_blocks() {
    let cli = Cli::try_parse_from([
        "solaris-v2",
        "--config",
        "probe.toml",
        "world",
        "probe",
        "--source",
        "world",
        "--receipt",
        "probe.json",
        "--block",
        "-574,71,320",
        "--block",
        "8,-60,8",
    ])
    .unwrap();
    assert_eq!(cli.config, PathBuf::from("probe.toml"));
    let Some(Command::World {
        command: WorldCommand::Probe(paths),
    }) = cli.command
    else {
        panic!("world probe")
    };
    assert_eq!(paths.source, PathBuf::from("world"));
    assert_eq!(paths.receipt, PathBuf::from("probe.json"));
    assert_eq!(paths.blocks, vec![[-574, 71, 320], [8, -60, 8]]);
    for invalid in [
        vec!["--source", "world", "--receipt", "probe.json"],
        vec![
            "--source",
            "world",
            "--receipt",
            "probe.json",
            "--block",
            "1,2",
        ],
        vec![
            "--source",
            "world",
            "--receipt",
            "probe.json",
            "--block",
            "1,2,3,4",
        ],
        vec![
            "--source",
            "world",
            "--receipt",
            "probe.json",
            "--block",
            "1,two,3",
        ],
    ] {
        let mut args = vec!["solaris-v2", "world", "probe"];
        args.extend(invalid);
        assert!(Cli::try_parse_from(args).is_err());
    }
}

#[test]
fn world_upgrade_worldgen_takes_only_the_config() {
    let cli = Cli::try_parse_from([
        "solaris-v2",
        "world",
        "upgrade-worldgen",
        "--config",
        "live.toml",
    ])
    .unwrap();
    assert_eq!(cli.config, PathBuf::from("live.toml"));
    assert!(matches!(
        cli.command,
        Some(Command::World {
            command: WorldCommand::UpgradeWorldgen
        })
    ));
    assert!(
        Cli::try_parse_from(["solaris-v2", "world", "upgrade-worldgen", "--source", "w"]).is_err()
    );
}

#[test]
fn world_pregenerate_takes_a_block_radius_and_optional_center() {
    let cli = Cli::try_parse_from([
        "solaris-v2",
        "world",
        "pregenerate",
        "--radius",
        "512",
        "--center",
        "-1024,2048",
        "--config",
        "server.toml",
    ])
    .unwrap();
    assert_eq!(cli.config, PathBuf::from("server.toml"));
    let Some(Command::World {
        command: WorldCommand::Pregenerate { radius, center },
    }) = cli.command
    else {
        panic!("world pregenerate")
    };
    assert_eq!(radius, 512);
    assert_eq!(center, Some([-1024, 2048]));

    let cli =
        Cli::try_parse_from(["solaris-v2", "world", "pregenerate", "--radius", "16"]).unwrap();
    assert!(matches!(
        cli.command,
        Some(Command::World {
            command: WorldCommand::Pregenerate {
                radius: 16,
                center: None
            }
        })
    ));

    for broken in [
        vec!["world", "pregenerate"],
        vec!["world", "pregenerate", "--radius", "0"],
        vec!["world", "pregenerate", "--radius", "16", "--center", "1"],
        vec!["world", "pregenerate", "--radius", "16", "--center", "1,"],
        vec!["world", "pregenerate", "--radius", "16", "--center", "a,b"],
        vec!["world", "pregenerate", "--radius", "16", "--source", "w"],
    ] {
        let mut args = vec!["solaris-v2"];
        args.extend(broken);
        assert!(Cli::try_parse_from(args).is_err());
    }
}

#[test]
fn world_prune_needs_a_keep_radius_and_defaults_to_never_inhabited_chunks() {
    let cli = Cli::try_parse_from([
        "solaris-v2",
        "world",
        "prune",
        "--keep-radius",
        "1024",
        "--center",
        "-64,32",
        "--dry-run",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Command::World {
            command: WorldCommand::Prune {
                keep_radius: 1024,
                center: Some([-64, 32]),
                max_inhabited_ticks: 0,
                dry_run: true,
            }
        })
    ));
    let cli = Cli::try_parse_from([
        "solaris-v2",
        "world",
        "prune",
        "--keep-radius",
        "0",
        "--max-inhabited-ticks",
        "20",
    ])
    .unwrap();
    assert!(matches!(
        cli.command,
        Some(Command::World {
            command: WorldCommand::Prune {
                keep_radius: 0,
                center: None,
                max_inhabited_ticks: 20,
                dry_run: false,
            }
        })
    ));
    for broken in [
        vec!["world", "prune"],
        vec!["world", "prune", "--keep-radius", "-1"],
        vec![
            "world",
            "prune",
            "--keep-radius",
            "16",
            "--max-inhabited-ticks",
            "-1",
        ],
        vec!["world", "prune", "--keep-radius", "16", "--center", "1"],
    ] {
        let mut args = vec!["solaris-v2"];
        args.extend(broken);
        assert!(Cli::try_parse_from(args.clone()).is_err(), "{args:?}");
    }
    assert!(matches!(
        Cli::try_parse_from(["solaris-v2", "world", "compact"])
            .unwrap()
            .command,
        Some(Command::World {
            command: WorldCommand::Compact
        })
    ));
}

#[test]
fn serving_syntax_keeps_no_console_flag() {
    let cli =
        Cli::try_parse_from(["solaris-v2", "--config", "serve.toml", "--no-console"]).unwrap();
    assert_eq!(cli.config, PathBuf::from("serve.toml"));
    assert!(cli.no_console);
    assert!(cli.command.is_none());
    let cli = Cli::try_parse_from(["solaris-v2"]).unwrap();
    assert!(!cli.no_console);
    assert_eq!(cli.config, PathBuf::from("server.toml"));
}

#[tokio::test]
async fn offline_verify_missing_cache_reports_import_instruction_without_opening_world() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing-cache");
    let cli = Cli::try_parse_from([
        "solaris-v2",
        "content",
        "verify",
        "--cache",
        missing.to_str().unwrap(),
    ])
    .unwrap();
    let mut cfg = crate::config::EdgeConfig::default();
    cfg.data.world_dir = dir.path().join("world");
    let error = format!("{:#}", run_offline(&cli, &mut cfg).await.unwrap_err());
    assert!(error.contains("content import"), "{error}");
    assert!(!cfg.data.world_dir.exists());
}

#[test]
fn path_only_offline_commands_do_not_require_a_serving_config() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("missing.toml");
    for command in [
        vec!["backup", "--world-dir", "world", "--out", "backup"],
        vec!["restore", "--from", "backup", "--world-dir", "world"],
        vec!["content", "verify", "--cache", "cache"],
        vec![
            "content",
            "import",
            "--from",
            "server.jar",
            "--cache",
            "cache",
        ],
    ] {
        let mut args = vec!["solaris-v2", "--config", missing.to_str().unwrap()];
        args.extend(command);
        assert!(load_config(&Cli::try_parse_from(args).unwrap()).is_ok());
    }
    assert!(
        load_config(
            &Cli::try_parse_from(["solaris-v2", "--config", missing.to_str().unwrap(),]).unwrap()
        )
        .is_err()
    );
    let invalid = dir.path().join("invalid.toml");
    std::fs::write(&invalid, "not toml = [").unwrap();
    let cli = Cli::try_parse_from([
        "solaris-v2",
        "--config",
        invalid.to_str().unwrap(),
        "backup",
        "--world-dir",
        "world",
        "--out",
        "backup",
    ])
    .unwrap();
    assert!(load_config(&cli).is_ok());
    let cli = Cli::try_parse_from([
        "solaris-v2",
        "--config",
        invalid.to_str().unwrap(),
        "content",
        "verify",
    ])
    .unwrap();
    assert!(load_config(&cli).is_err());
}
