use std::io::Write;

use super::*;

fn declared(index: &str) -> Result<Vec<mc_script::ScriptClientViewKey>, LoaderHandshakeError> {
    let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
    archive
        .start_file(
            LOADER_ARTIFACT_INDEX_PATH,
            zip::write::SimpleFileOptions::default(),
        )
        .unwrap();
    archive.write_all(index.as_bytes()).unwrap();
    let bytes = archive.finish().unwrap().into_inner();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let index = read_index_from_artifact_bytes(
        &bytes,
        Path::new("fixture.bundle"),
        bytes.len() as u64,
        &sha256,
    )?;
    declared_screen_keys(&index, "example")
}

#[test]
fn arbitrary_declared_screen_ids_are_request_keys() {
    assert_eq!(
        declared(
            r#"{"schema":2,"screens":[
                {"id":"example:panel/main"},
                {"id":"example:overlay","overlay":true}
            ]}"#,
        )
        .unwrap(),
        vec![
            mc_script::ScriptClientViewKey::try_new("example:panel/main").unwrap(),
            mc_script::ScriptClientViewKey::try_new("example:overlay").unwrap(),
        ]
    );
}

#[test]
fn invalid_screen_declarations_still_fail_before_routing() {
    for index in [
        r#"{"schema":2,"screens":[]}"#,
        r#"{"schema":2,"screens":[{"id":"panel"}]}"#,
        r#"{"schema":2,"screens":[{"id":"example:Panel"}]}"#,
        r#"{"schema":2,"screens":[{"id":"other:panel"}]}"#,
        r#"{"schema":2,"screens":[{"id":"example:"}]}"#,
    ] {
        assert!(matches!(
            declared(index),
            Err(LoaderHandshakeError::ArtifactIndex(_))
        ));
    }
}
