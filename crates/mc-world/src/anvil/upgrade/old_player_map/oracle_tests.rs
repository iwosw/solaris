//! Opt-in local DFU differential gate. Input/oracle bytes remain outside the repository.
use std::{fs, path::PathBuf};

use mc_nbt::Tag;
use serde::Deserialize;

use super::super::{TARGET_DATA_VERSION, nbt, upgrade_map_data, upgrade_player};

#[derive(Deserialize)]
struct Case {
    kind: String,
    input: PathBuf,
    output: PathBuf,
    from: i32,
    solaris_output: PathBuf,
    /// Only Zombie502's approved random-profession branch has six exact
    /// vanilla post502 control outputs; no fields are masked for comparison.
    #[serde(default)]
    alternatives: Vec<PathBuf>,
}

fn read(path: &std::path::Path) -> Tag {
    let bytes = fs::read(path).unwrap();
    let mut cursor = bytes.as_slice();
    let (_, tag) = mc_nbt::read_named(&mut cursor).unwrap();
    assert!(cursor.is_empty(), "trailing NBT in {}", path.display());
    tag
}

fn canonical(tag: &mut Tag) {
    match tag {
        Tag::Compound(fields) => {
            fields.sort_by(|left, right| left.0.cmp(&right.0));
            for (_, value) in fields {
                canonical(value);
            }
        }
        Tag::List(list) => {
            if list.elements.is_empty() {
                list.element_type = 0;
            }
            for value in &mut list.elements {
                canonical(value);
            }
        }
        _ => {}
    }
}

#[test]
#[ignore = "requires local official-jar DFU manifest in SOLARIS_LEGACY_ORACLE_MANIFEST"]
fn legacy_player_map_local_dfu_oracle() {
    let manifest =
        PathBuf::from(std::env::var_os("SOLARIS_LEGACY_ORACLE_MANIFEST").expect("oracle manifest"));
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(&manifest).unwrap()).unwrap();
    assert!(!cases.is_empty(), "empty oracle manifest");
    let mut results = Vec::new();
    let mut failures = 0;
    for case in &cases {
        assert!(
            case.alternatives.is_empty()
                || (case.kind == "player" && case.from == 501 && case.alternatives.len() == 6),
            "only the six Zombie502 outcomes may supply alternative oracle outputs"
        );
        let mut actual = read(&case.input);
        let upgraded = match case.kind.as_str() {
            "player" => upgrade_player(&mut actual, case.from),
            "map" => upgrade_map_data(&mut actual, case.from),
            kind => panic!("unsupported oracle case kind {kind}"),
        };
        let status = match upgraded {
            Err(error) => {
                failures += 1;
                serde_json::json!({"status":"upgrade_error","error":error.to_string()})
            }
            Ok(report) => {
                assert_eq!(report.from, case.from);
                assert_eq!(
                    nbt::get(&actual, "DataVersion"),
                    Some(&Tag::Int(TARGET_DATA_VERSION))
                );
                let mut bytes = Vec::new();
                mc_nbt::write_named(&mut bytes, "", &actual).unwrap();
                fs::write(&case.solaris_output, bytes).unwrap();
                let mut expected = read(&case.output);
                // DFU update does not own the storage layer's source-version stamp.
                nbt::remove(&mut actual, "DataVersion");
                nbt::remove(&mut expected, "DataVersion");
                canonical(&mut actual);
                canonical(&mut expected);
                if actual == expected {
                    serde_json::json!({"status":"equal","steps":report.steps.len()})
                } else {
                    let alternative = case.alternatives.iter().find(|path| {
                        let mut alternative = read(path);
                        nbt::remove(&mut alternative, "DataVersion");
                        canonical(&mut alternative);
                        actual == alternative
                    });
                    if let Some(path) = alternative {
                        serde_json::json!({
                            "status":"accepted_nondeterministic",
                            "fix":"EntityZombieVillagerTypeFix",
                            "vanilla":case.output,
                            "matched_vanilla_control":path,
                            "solaris":case.solaris_output
                        })
                    } else {
                        failures += 1;
                        serde_json::json!({"status":"different","vanilla":case.output,"solaris":case.solaris_output})
                    }
                }
            }
        };
        results.push(serde_json::json!({"kind":case.kind,"from":case.from,"input":case.input,"result":status}));
    }
    let receipt = manifest.with_extension("solaris-result.json");
    fs::write(
        &receipt,
        serde_json::to_vec_pretty(
            &serde_json::json!({"count":cases.len(),"failures":failures,"cases":results}),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(failures, 0, "oracle residues: {}", receipt.display());
}
