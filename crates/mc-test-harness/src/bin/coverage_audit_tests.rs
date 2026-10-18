use super::*;

#[test]
fn partial_rows_do_not_count_even_with_harness_words() {
    let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `partial` | Harness and vanilla capture exist. | More cases missing. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

    let result = audit_ledger(markdown).expect("audit should parse");

    assert_eq!(result.denominator, 1);
    assert_eq!(result.numerator, 0);
}

#[test]
fn ready_rows_require_runtime_and_vanilla_or_client_evidence() {
    let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `ready` | Runtime harness plus vanilla oracle capture. | None for normal path. | M95 |
| B2 | Doors | In scope | `ready` | Unit tests only. | No client evidence. | M95 |
| B3 | Fluids | In scope | `ready` | Runtime harness. | No oracle evidence. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

    let result = audit_ledger(markdown).expect("audit should parse");

    assert_eq!(result.denominator, 3);
    assert_eq!(result.numerator, 1);
    assert!(result.rows[0].counts);
    assert!(!result.rows[1].counts);
    assert!(!result.rows[2].counts);
}

#[test]
fn ready_rows_do_not_count_negated_or_unit_only_evidence() {
    let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `ready` | Runtime harness and unit tests only. | No oracle evidence. | M95 |
| B2 | Doors | In scope | `ready` | Runtime harness without vanilla evidence. | None for normal path. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

    let result = audit_ledger(markdown).expect("audit should parse");

    assert_eq!(result.denominator, 2);
    assert_eq!(result.numerator, 0);
    assert!(!result.rows[0].counts);
    assert!(!result.rows[1].counts);
}

#[test]
fn wire_probe_only_does_not_satisfy_both_evidence_legs() {
    let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| C1 | Chunk streaming | In scope | `ready` | Long wire-probe runtime test. | None for normal path. | M95 |
| C2 | Lighting payloads | In scope | `ready` | Wire-probe runtime test plus vanilla capture. | None for normal path. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

    let result = audit_ledger(markdown).expect("audit should parse");

    assert_eq!(result.denominator, 2);
    assert_eq!(result.numerator, 1);
    assert!(!result.rows[0].counts);
    assert!(result.rows[1].counts);
}

#[test]
fn negated_or_unlinked_oracle_client_phrases_fail_closed() {
    let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `ready` | Runtime harness lacks oracle evidence. | None for normal path. | M95 |
| B2 | Doors | In scope | `ready` | Runtime harness has no linked vanilla oracle evidence. | None for normal path. | M95 |
| B3 | Fluids | In scope | `ready` | Runtime harness has no vanilla evidence. | None for normal path. | M95 |
| B4 | Movement | In scope | `ready` | Runtime harness has no oracle evidence. | None for normal path. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

    let result = audit_ledger(markdown).expect("audit should parse");

    assert_eq!(result.denominator, 4);
    assert_eq!(result.numerator, 0);
    assert!(result.rows.iter().all(|row| !row.counts));
}

#[test]
fn bare_or_non_vanilla_captures_do_not_count_as_oracle_or_client_evidence() {
    let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| C1 | Chunk streaming | In scope | `ready` | Runtime harness plus capture. | None for normal path. | M95 |
| C2 | Lighting payloads | In scope | `ready` | Runtime harness plus wire-probe capture. | None for normal path. | M95 |
| C3 | Collision | In scope | `ready` | Runtime harness plus Solaris harness capture. | None for normal path. | M95 |
| C4 | Falling blocks | In scope | `ready` | Runtime harness plus vanilla capture. | None for normal path. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

    let result = audit_ledger(markdown).expect("audit should parse");

    assert_eq!(result.denominator, 4);
    assert_eq!(result.numerator, 1);
    assert!(!result.rows[0].counts);
    assert!(!result.rows[1].counts);
    assert!(!result.rows[2].counts);
    assert!(result.rows[3].counts);
}

#[test]
fn malformed_denominator_rows_fail_loudly() {
    let markdown = r#"
## Frozen M100 Denominator

| ID | Row | M100 scope | Status | Current evidence | Known gaps and debt | Target milestones |
|---|---|---|---|---|---|---|
| B1 | Block edits | In scope | `ready` | Runtime harness | vanilla capture. | None. | M95 |

## Accepted Non-Goals And Divergences Appendix
"#;

    audit_ledger(markdown).expect_err("row should be malformed");
}
