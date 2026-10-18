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
            WorldCommand::Census(_) => unreachable!("conversion names only"),
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
