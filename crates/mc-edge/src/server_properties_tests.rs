use super::*;

#[test]
fn serverops_java_properties_escapes_continuations_and_duplicates() {
    let parsed = parse("# comment\n! comment\nmotd : Hello\\u0020world\\\n  again\nkey\\:x=value\\=two\na=first\na=last\nemoji=\\uD83D\\uDE00\n").unwrap();
    assert_eq!(parsed["motd"], "Hello worldagain");
    assert_eq!(parsed["key:x"], "value=two");
    assert_eq!(parsed["a"], "last");
    assert_eq!(parsed["emoji"], "\u{1f600}");
    assert!(parse("x=\\u00QQ").is_err());
    assert_eq!(parse("\\\n#key=value").unwrap()["#key"], "value");
    assert_eq!(
        boolean("online-mode", "TRUE").unwrap(),
        Value::Boolean(true)
    );
}

#[test]
fn serverops_properties_import_precedence_and_services() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("server.properties"), "motd=Imported\nserver-port=25590\nserver-ip=\nonline-mode=true\nview-distance=12\nlevel-seed=Hello\nwhite-list=true\nenable-rcon=true\nrcon.password=secret\nenable-query=true\nquery.port=25591\nresource-pack=https\\://example.org/pack.zip\nrequire-resource-pack=true\nresource-pack-prompt={\"text\":\"Install\"}\nunsupported-key=nope\n").unwrap();
    let config = load(
        "[server]\nmotd='Canonical'\n",
        &root.path().join("server.toml"),
    )
    .unwrap();
    assert_eq!(config.server.motd, "Canonical");
    assert_eq!(config.network.port, 25590);
    assert_eq!(config.network.bind_address, "0.0.0.0");
    assert!(config.auth.online_mode);
    assert_eq!(config.server.view_distance, 12);
    assert_eq!(config.data.seed, 69609650);
    assert_eq!(config.server.whitelist, Some(true));
    assert!(config.rcon.enabled && config.query.enabled && config.resource_pack.required);
    assert_eq!(config.rcon.password, "secret");
    assert_eq!(config.query.port, 25591);
    assert_eq!(config.resource_pack.url, "https://example.org/pack.zip");
}

#[test]
fn serverops_properties_explicit_path_and_errors() {
    let root = tempfile::tempdir().unwrap();
    let origin = root.path().join("server.toml");
    assert!(load("", &origin).is_ok());
    assert!(load("[properties]\nfile='missing.properties'", &origin).is_err());
    std::fs::write(root.path().join("custom.properties"), "max-players=7\n").unwrap();
    assert_eq!(
        load("[properties]\nfile='custom.properties'", &origin)
            .unwrap()
            .server
            .max_players,
        7
    );
    std::fs::write(
        root.path().join("server.properties"),
        "server-port=not-a-port",
    )
    .unwrap();
    assert!(load("", &origin).is_err());
}

#[test]
fn serverops_configuration_services_are_opt_in_and_validate_secrets() {
    let default = crate::config::EdgeConfig::default();
    assert!(
        !default.rcon.enabled && !default.query.enabled && default.resource_pack.url.is_empty()
    );
    assert!(crate::config::EdgeConfig::from_toml("[rcon]\nenabled=true", None).is_err());
    assert!(crate::config::EdgeConfig::from_toml("[resource_pack]\nrequired=true", None).is_err());
}
