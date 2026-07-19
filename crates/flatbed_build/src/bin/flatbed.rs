//! `flatbed` — the FlatBuffer codegen helper.
//!
//! A thin CLI around [`flatbed_build::Config`] so codegen can be
//! driven directly — without compiling the full workspace — by
//! passing a schemas directory and an output path.
//!
//! Two subcommands: `generate` walks a schemas directory non-recursively
//! and runs every top-level `.fbs` file through `Config::compile`
//! (subdirectories like `v1/` are reached via FlatBuffer `include` from the
//! roots); `gen-fb-plugin` cross-checks a served OpenAPI spec against those
//! same schemas before a FlatBuffer client is generated.

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
    /// Cross-check a served OpenAPI spec against local `.fbs` schemas: every
    /// `application/x-flatbuffers` operation's request/response types must be
    /// present in `--schemas-dir`, or the local schemas are out of sync with
    /// the deployed server.
    GenFbPlugin {
        /// Base URL of a running flatbed server; its `/openapi.json` is fetched.
        #[arg(long, conflicts_with = "openapi")]
        server: Option<String>,
        /// A saved OpenAPI spec file, read instead of fetching from a server.
        #[arg(long, conflicts_with = "server")]
        openapi: Option<PathBuf>,
        /// Directory of `.fbs` schemas to validate the spec against.
        #[arg(long)]
        schemas_dir: PathBuf,
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
        Command::GenFbPlugin {
            server,
            openapi,
            schemas_dir,
        } => {
            let source = match (server, openapi) {
                (Some(url), None) => flatbed_build::SpecSource::Server(url),
                (None, Some(file)) => flatbed_build::SpecSource::File(file),
                (None, None) => {
                    eprintln!(
                        "flatbed gen-fb-plugin: provide exactly one of --server or --openapi"
                    );
                    return ExitCode::FAILURE;
                }
                (Some(_), Some(_)) => {
                    unreachable!("clap's conflicts_with rejects --server together with --openapi")
                }
            };
            match flatbed_build::gen_fb_plugin(source, &schemas_dir) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("flatbed gen-fb-plugin failed: {e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

fn run_generate(schemas_dir: &Path, out: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let schemas = flatbed_build::root_fbs_files(schemas_dir)?;
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
