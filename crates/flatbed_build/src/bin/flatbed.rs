//! `flatbed` — the FlatBuffer codegen helper.
//!
//! A thin CLI around [`flatbed_build::Config`] so codegen can be
//! driven directly — without compiling the full workspace — by
//! passing a schemas directory and an output path.
//!
//! The binary exposes one subcommand, `generate`, that walks a
//! schemas directory non-recursively and runs every top-level `.fbs`
//! file through `Config::compile`. Subdirectories (`v1/`, `v2/`, …)
//! are reached via FlatBuffer `include` directives from the root
//! files, the same way the operator's `build.rs` drives them.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "flatbed", about = "FlatBuffer codegen helper")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Regenerate `<stem>_generated.rs` + `<stem>_flatbed.rs` from
    /// every top-level `.fbs` in `--schemas-dir`. Output lands in
    /// `--out`. Subdirectory schemas (e.g. `v1/`) are reached via
    /// FlatBuffer `include` directives in the root files.
    Generate {
        /// Directory containing the root `.fbs` files to compile.
        /// Subdirectories are not walked.
        #[arg(long)]
        schemas_dir: PathBuf,
        /// Output directory for generated `.rs` and `.bfbs` files.
        /// Created if missing.
        #[arg(long)]
        out: PathBuf,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Generate { schemas_dir, out } => match run_generate(&schemas_dir, &out) {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("flatbed generate failed: {e}");
                ExitCode::FAILURE
            }
        },
    }
}

fn run_generate(schemas_dir: &Path, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let schemas = discover_root_schemas(schemas_dir)?;
    if schemas.is_empty() {
        return Err(format!(
            "no top-level .fbs files found in {} — codegen has nothing to do",
            schemas_dir.display()
        )
        .into());
    }

    std::fs::create_dir_all(out)
        .map_err(|e| format!("failed to create output directory {}: {e}", out.display()))?;

    let config = schemas
        .iter()
        .fold(flatbed_build::Config::new().out_dir(out), |c, s| {
            c.schema(s)
        });
    config.compile()?;

    println!(
        "flatbed: regenerated {} schema(s) from {} into {}",
        schemas.len(),
        schemas_dir.display(),
        out.display(),
    );
    Ok(())
}

/// Return the sorted set of `.fbs` files at the top level of `dir`.
/// Subdirectories are intentionally ignored — the convention is that
/// root files live in `<schemas-dir>/*.fbs` and pull in versioned
/// schemas via FlatBuffer `include` from subdirs like `v1/`. Sorting
/// gives the generator a deterministic order so re-runs against the
/// same inputs emit byte-identical output.
fn discover_root_schemas(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut roots: Vec<PathBuf> = std::fs::read_dir(dir)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("fbs"))
        .collect();
    roots.sort();
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// Build a temp tree containing the given relative filenames as
    /// empty files. Caller owns the returned path; the directory
    /// persists in `/tmp` until the OS's temp-cleanup job sweeps it.
    /// Test runs leave behind a handful of small empty dirs, which
    /// the platform's tmpwatch / systemd-tmpfiles reaps in due
    /// course — fine for a test scratch helper.
    ///
    /// Uses an atomic counter (process-id + monotone index) so
    /// parallel `cargo test` threads never land on the same directory,
    /// keeping each test's filesystem state isolated.
    fn make_tree(files: &[&str]) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let base = std::env::temp_dir().join(format!(
            "flatbed_test_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed),
        ));
        std::fs::create_dir_all(&base).unwrap();
        for rel in files {
            let p = base.join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&p, "").unwrap();
        }
        base
    }

    #[test]
    fn discover_picks_up_top_level_fbs_files() {
        let dir = make_tree(&["operator.fbs", "user.fbs"]);
        let found = discover_root_schemas(&dir).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["operator.fbs", "user.fbs"]);
    }

    #[test]
    fn discover_ignores_subdirectory_fbs_files() {
        // Versioned schemas under `v1/` are reached via `include`
        // from the root files; the binary must NOT compile them as
        // their own root or the operator's compiled output would
        // double up (root + version) and the diff against the
        // committed `_generated.rs` would never converge.
        let dir = make_tree(&["operator.fbs", "v1/operator.fbs", "v1/user.fbs"]);
        let found = discover_root_schemas(&dir).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["operator.fbs"]);
    }

    #[test]
    fn discover_ignores_non_fbs_files() {
        let dir = make_tree(&["operator.fbs", "README.md", "operator.fbs.bak"]);
        let found = discover_root_schemas(&dir).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["operator.fbs"]);
    }

    #[test]
    fn discover_returns_sorted_order() {
        // Sort order is what makes the codegen deterministic across
        // re-runs. Without it, filesystem readdir order would leak
        // into the output's `mod` declaration order and produce
        // spurious diffs on every run on a different OS.
        let dir = make_tree(&["zzz.fbs", "aaa.fbs", "mmm.fbs"]);
        let found = discover_root_schemas(&dir).unwrap();
        let names: Vec<_> = found
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect();
        assert_eq!(names, vec!["aaa.fbs", "mmm.fbs", "zzz.fbs"]);
    }
}
