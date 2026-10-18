use std::env;
use std::fs;
use std::path::PathBuf;
use std::process;

const DEFAULT_LEDGER: &str = "docs/VALIDATION_LEDGER.md";

#[derive(Debug, Clone, PartialEq, Eq)]
struct LedgerRow {
    id: String,
    row: String,
    scope: String,
    status: String,
    evidence: String,
    gaps: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AuditResult {
    denominator: usize,
    numerator: usize,
    rows: Vec<RowAudit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RowAudit {
    id: String,
    row: String,
    status: String,
    counts: bool,
    reason: String,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("coverage-audit: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let ledger_path = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_LEDGER));
    if let Some(extra) = args.next() {
        return Err(format!(
            "unexpected argument `{extra}`; usage: coverage-audit [ledger.md]"
        ));
    }

    let ledger = fs::read_to_string(&ledger_path).map_err(|error| {
        format!(
            "failed to read validation ledger `{}`: {error}",
            ledger_path.display()
        )
    })?;
    let result = audit_ledger(&ledger)?;

    println!("# Validation Coverage Audit");
    println!();
    println!("Ledger: `{}`", ledger_path.display());
    println!("Denominator: {} in-scope rows", result.denominator);
    println!("Numerator: {} conservative ready rows", result.numerator);
    println!(
        "Coverage: {:.2}%",
        coverage_percent(result.numerator, result.denominator)
    );
    println!();
    println!("| ID | Status | Counts | Reason |");
    println!("|---|---|---|---|");
    for row in result.rows {
        println!(
            "| {} | `{}` | {} | {} |",
            row.id,
            row.status,
            if row.counts { "yes" } else { "no" },
            row.reason
        );
    }

    Ok(())
}

fn audit_ledger(markdown: &str) -> Result<AuditResult, String> {
    let rows = parse_frozen_denominator(markdown)?;
    let mut audited = Vec::with_capacity(rows.len());
    let mut numerator = 0;

    for row in rows {
        let (counts, reason) = count_reason(&row);
        if counts {
            numerator += 1;
        }
        audited.push(RowAudit {
            id: row.id,
            row: row.row,
            status: row.status,
            counts,
            reason,
        });
    }

    Ok(AuditResult {
        denominator: audited.len(),
        numerator,
        rows: audited,
    })
}

fn parse_frozen_denominator(markdown: &str) -> Result<Vec<LedgerRow>, String> {
    let mut in_table = false;
    let mut rows = Vec::new();

    for (line_index, line) in markdown.lines().enumerate() {
        if line.trim() == "## Frozen M100 Denominator" {
            in_table = true;
            continue;
        }

        if in_table && line.starts_with("## ") {
            break;
        }

        if !in_table || !line.starts_with('|') {
            continue;
        }

        let columns = split_markdown_row(line);
        if columns[0] == "ID" || columns[0].starts_with("---") {
            continue;
        }
        if columns.len() != 7 {
            return Err(format!(
                "malformed frozen denominator row at line {}: expected 7 columns, found {}; remove internal `|` characters from the table row",
                line_index + 1,
                columns.len()
            ));
        }
        if columns[2] != "In scope" {
            continue;
        }

        rows.push(LedgerRow {
            id: columns[0].to_owned(),
            row: columns[1].to_owned(),
            scope: columns[2].to_owned(),
            status: unquote_status(columns[3]),
            evidence: columns[4].to_owned(),
            gaps: columns[5].to_owned(),
        });
    }

    if !in_table {
        return Err("missing `## Frozen M100 Denominator` section".to_owned());
    }
    if rows.is_empty() {
        return Err("frozen denominator table has no in-scope rows".to_owned());
    }

    Ok(rows)
}

fn split_markdown_row(line: &str) -> Vec<&str> {
    line.trim_matches('|').split('|').map(str::trim).collect()
}

fn unquote_status(status: &str) -> String {
    status.trim_matches('`').to_owned()
}

fn count_reason(row: &LedgerRow) -> (bool, String) {
    if row.status != "ready" {
        return (false, format!("status `{}` is not `ready`", row.status));
    }

    let evidence = row.evidence.to_ascii_lowercase();
    let gaps = row.gaps.to_ascii_lowercase();
    if has_disqualifying_evidence(&evidence) || has_disqualifying_evidence(&gaps) {
        return (
            false,
            "mentions unit-only, negated, Solaris-only, missing, blocked, or manual-pending evidence"
                .to_owned(),
        );
    }

    if !has_runtime_evidence(&evidence) {
        return (false, "no focused runtime test evidence found".to_owned());
    }

    if !has_vanilla_or_client_evidence(&evidence) {
        return (
            false,
            "no vanilla oracle or real-client evidence found".to_owned(),
        );
    }

    (
        true,
        "ready with runtime plus vanilla/client evidence".to_owned(),
    )
}

fn has_runtime_evidence(evidence: &str) -> bool {
    ["harness", "runtime", "test", "wire-probe", "prismlauncher"]
        .iter()
        .any(|needle| evidence.contains(needle))
}

fn has_vanilla_or_client_evidence(evidence: &str) -> bool {
    [
        "real-client",
        "real client",
        "prismlauncher",
        "vanilla oracle",
        "vanilla-oracle",
        "vanilla server oracle",
        "vanilla capture",
        "vanilla-capture",
    ]
    .iter()
    .any(|needle| evidence.contains(needle))
}

fn has_disqualifying_evidence(text: &str) -> bool {
    [
        "lacks oracle",
        "lacks client",
        "lacks real-client",
        "lacks real client",
        "lacks vanilla",
        "unit-only",
        "unit tests only",
        "wire-probe-only",
        "wire-probe capture",
        "solaris-only",
        "solaris harness capture",
        "manual-pending",
        "not run",
        "missing",
        "blocked",
        "lacks required",
        "no linked",
        "no linked oracle",
        "no linked client",
        "no linked vanilla",
        "no oracle",
        "no client evidence",
        "no real-client",
        "no real client",
        "no vanilla",
        "without oracle",
        "without client evidence",
        "without real-client",
        "without real client",
        "without vanilla",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

fn coverage_percent(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        0.0
    } else {
        (numerator as f64 / denominator as f64) * 100.0
    }
}

#[cfg(test)]
#[path = "coverage_audit_tests.rs"]
mod tests;
