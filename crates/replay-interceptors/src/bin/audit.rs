//! replay-audit — automated codebase scanner.
//!
//! Walks Rust source files and detects call sites that should be wrapped by
//! replay-compat to participate in record/replay:
//!
//!   • `reqwest::Client::new()` / `reqwest::Client::builder()`
//!   • `tokio::spawn(…)`
//!   • `redis::Client::open(…)`
//!   • `sqlx::Pool::connect(…)` / `sqlx::PgPool::connect(…)`
//!
//! With `--fix` the tool rewrites the source in-place to use the replay_compat
//! equivalents.  With `--json` it emits a machine-readable JSON report.
//!
//! # Usage
//!
//! ```text
//! replay-audit [OPTIONS] [PATH]
//!
//! Arguments:
//!   [PATH]  Directory to scan (default: current directory)
//!
//! Options:
//!   --fix          Rewrite sources in-place (experimental)
//!   --json         Emit JSON report to stdout
//!   --ignore <PAT> Substring pattern to exclude paths (repeatable)
//!   -h, --help     Print this help
//! ```

use std::{
    fs,
    path::{Path, PathBuf},
};

use clap::Parser;
use walkdir::WalkDir;

// ── CLI ───────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(
    name = "replay-audit",
    about = "Scan Rust source for call sites that should use replay_compat"
)]
struct Cli {
    /// Directory or file to scan (default: current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Rewrite sources in-place to use replay_compat equivalents
    #[arg(long)]
    fix: bool,

    /// Emit JSON report to stdout instead of human-readable output
    #[arg(long)]
    json: bool,

    /// Substring patterns to exclude from scanning (repeatable)
    #[arg(long = "ignore", value_name = "PAT")]
    ignore: Vec<String>,
}

// ── findings ──────────────────────────────────────────────────────────────────

#[derive(Debug, serde::Serialize)]
struct Finding {
    file:    PathBuf,
    line:    usize,
    kind:    &'static str,
    snippet: String,
    fix:     &'static str,
}

/// Each pattern: (display name, string to look for in source, suggestion)
static PATTERNS: &[(&str, &str, &str)] = &[
    (
        "reqwest::Client::new()",
        "reqwest::Client::new()",
        "use replay_compat::http::Client::new() instead",
    ),
    (
        "reqwest::Client::builder()",
        "reqwest::Client::builder()",
        "use replay_compat::http::Client::new() instead",
    ),
    (
        "tokio::task::spawn(",
        "tokio::task::spawn(",
        "use replay_compat::tokio::task::spawn(...) or annotate with #[instrument_spawns]",
    ),
    (
        "tokio::spawn(",
        "tokio::spawn(",
        "use replay_compat::tokio::task::spawn(...) or annotate with #[instrument_spawns]",
    ),
    (
        "redis::Client::open(",
        "redis::Client::open(",
        "use replay_compat::redis::open(...) instead",
    ),
    (
        "sqlx::PgPool::connect(",
        "sqlx::PgPool::connect(",
        "use replay_compat::sql::connect(...) instead",
    ),
    (
        "sqlx::Pool::connect(",
        "sqlx::Pool::connect(",
        "use replay_compat::sql::connect(...) instead",
    ),
    (
        "sqlx::MySqlPool::connect(",
        "sqlx::MySqlPool::connect(",
        "use replay_compat::sql::connect(...) instead",
    ),
];

// ── scanning ──────────────────────────────────────────────────────────────────

fn scan_file(path: &Path) -> Vec<Finding> {
    let source = match fs::read_to_string(path) {
        Ok(s)  => s,
        Err(_) => return vec![],
    };

    // Skip generated / vendored files.
    if source.starts_with("// @generated") || source.starts_with("/* @generated") {
        return vec![];
    }

    let mut findings = Vec::new();

    for (line_no, line) in source.lines().enumerate() {
        let trimmed = line.trim();

        // Skip comments.
        if trimmed.starts_with("//") || trimmed.starts_with('*') {
            continue;
        }

        for &(kind, pattern, fix) in PATTERNS {
            if line.contains(pattern) {
                // Don't flag already-replaced lines.
                if line.contains("replay_compat") {
                    continue;
                }
                findings.push(Finding {
                    file:    path.to_path_buf(),
                    line:    line_no + 1,
                    kind,
                    snippet: trimmed.to_string(),
                    fix,
                });
                break; // one finding per line is enough
            }
        }
    }

    findings
}

fn should_ignore(path: &Path, patterns: &[String]) -> bool {
    let path_str = path.to_string_lossy();
    patterns.iter().any(|pat| path_str.contains(pat.as_str()))
}

// ── fix mode ──────────────────────────────────────────────────────────────────

fn fix_file(path: &Path) -> anyhow::Result<bool> {
    let source = fs::read_to_string(path)?;

    // Apply rewrites in order (most specific first to avoid double-replace).
    let rewritten = source
        .replace("reqwest::Client::new()",    "replay_compat::http::Client::new()")
        .replace("reqwest::Client::builder()", "replay_compat::http::Client::new()")
        .replace("redis::Client::open(",      "replay_compat::redis::open(")
        .replace("sqlx::PgPool::connect(",    "replay_compat::sql::connect(")
        .replace("sqlx::MySqlPool::connect(", "replay_compat::sql::connect(")
        .replace("sqlx::Pool::connect(",      "replay_compat::sql::connect(")
        // Rewrite tokio::task::spawn first, then bare tokio::spawn
        .replace("tokio::task::spawn(", "replay_compat::tokio::task::spawn(")
        .replace("tokio::spawn(",        "replay_compat::tokio::task::spawn(");

    if rewritten == source {
        return Ok(false);
    }

    // Format through prettyplease when possible.
    let formatted = match syn::parse_str::<syn::File>(&rewritten) {
        Ok(ast) => prettyplease::unparse(&ast),
        Err(_)  => rewritten,
    };

    fs::write(path, formatted)?;
    Ok(true)
}

// ── main ──────────────────────────────────────────────────────────────────────

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let rust_files: Vec<PathBuf> = WalkDir::new(&cli.path)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let p = e.path();
            p.extension().map(|x| x == "rs").unwrap_or(false)
                && !should_ignore(p, &cli.ignore)
        })
        .map(|e| e.into_path())
        .collect();

    if cli.fix {
        let mut count = 0;
        for path in &rust_files {
            match fix_file(path) {
                Ok(true)  => { eprintln!("fixed: {}", path.display()); count += 1; }
                Ok(false) => {}
                Err(e)    => eprintln!("warn: {}: {e}", path.display()),
            }
        }
        eprintln!("replay-audit --fix: rewrote {} file(s)", count);
        return Ok(());
    }

    let findings: Vec<Finding> = rust_files.iter().flat_map(|p| scan_file(p)).collect();

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&findings)?);
        return Ok(());
    }

    if findings.is_empty() {
        println!("replay-audit: no uninstrumented call sites found.");
        return Ok(());
    }

    for f in &findings {
        eprintln!("{}:{}: [{}]", f.file.display(), f.line, f.kind);
        eprintln!("  {}", f.snippet);
        eprintln!("  hint: {}", f.fix);
    }
    eprintln!();
    eprintln!("replay-audit: {} finding(s) — run with --fix to auto-rewrite", findings.len());

    std::process::exit(1);
}
