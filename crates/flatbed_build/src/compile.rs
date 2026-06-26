//! Driver: configure schemas, invoke `flatc` twice per file (once for the
//! FlatBuffer wire-format Rust, once for the `.bfbs` reflection binary), and
//! hand the reflected graph to the codegen.

use flatbuffers_reflection::reflection::root_as_schema;
use std::path::{Path, PathBuf};

use crate::codegen::generate_flatbed_module;
use crate::reflection::{build_reflected_schema, BFBS_FLATC_FLAGS, BFBS_ROOT_PREFIX};

/// Configuration builder for flatbed code generation
#[must_use = "Config does nothing until .compile() is called"]
pub struct Config {
    schemas: Vec<PathBuf>,
    out_dir: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self::new()
    }
}

impl Config {
    /// Create a new Config with default settings
    pub fn new() -> Self {
        Self {
            schemas: Vec::new(),
            out_dir: None,
        }
    }

    /// Add a FlatBuffer schema file to compile
    pub fn schema(mut self, path: impl AsRef<Path>) -> Self {
        self.schemas.push(path.as_ref().to_path_buf());
        self
    }

    /// Set the output directory (defaults to OUT_DIR)
    pub fn out_dir(mut self, path: impl AsRef<Path>) -> Self {
        self.out_dir = Some(path.as_ref().to_path_buf());
        self
    }

    /// Compile all configured schemas
    pub fn compile(self) -> Result<(), Box<dyn std::error::Error>> {
        let out_dir = self
            .out_dir
            .unwrap_or_else(|| PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set")));

        for schema_path in &self.schemas {
            compile_one_schema(schema_path, &out_dir)?;
        }

        Ok(())
    }
}

/// Compile a single schema: emit `_generated.rs` (FlatBuffer wire format),
/// then emit `<stem>.bfbs` (binary reflection schema) and walk it to drive
/// `_flatbed.rs` codegen.
///
/// The two flatc invocations cover orthogonal needs:
/// - `--rust` (default for `flatc_rust::run`) emits `_generated.rs`. On a
///   root file with `include` directives it only emits the root's own decls,
///   so multi-file schemas are concatenated from per-include runs.
/// - `-b --schema` (`binary: true, schema: true`) emits a `.bfbs` covering
///   the full include graph in one file. `flatbuffers-reflection` then
///   decodes it to drive codegen — no `.fbs` text parsing in the codegen
///   path.
fn compile_one_schema(
    schema_path: &Path,
    out_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let schema_dir = schema_path.parent().unwrap_or(Path::new("."));
    let stem = schema_path.file_stem().unwrap().to_str().unwrap();

    println!("cargo:rerun-if-changed={}", schema_path.display());

    // Emit `.bfbs` for codegen. flatc follows includes and writes a single
    // binary schema covering the full graph; we use the bfbs's `fbs_files`
    // list to discover transitive includes for rerun-if-changed below.
    let schema_dir_str = schema_dir
        .to_str()
        .ok_or("schema_dir contains invalid UTF-8")?;
    let mut bfbs_extra: Vec<&str> = BFBS_FLATC_FLAGS.to_vec();
    bfbs_extra.extend(["--bfbs-filenames", schema_dir_str]);
    flatc_rust::run(flatc_rust::Args {
        inputs: &[schema_path],
        out_dir,
        binary: true,
        schema: true,
        extra: &bfbs_extra,
        ..Default::default()
    })
    .map_err(|e| {
        format!(
            "failed to compile bfbs schema '{}': {}",
            schema_path.display(),
            e
        )
    })?;

    let bfbs_path = out_dir.join(format!("{}.bfbs", stem));
    let bfbs_bytes = std::fs::read(&bfbs_path).map_err(|e| {
        format!(
            "failed to read generated bfbs '{}': {}",
            bfbs_path.display(),
            e
        )
    })?;
    let schema = root_as_schema(&bfbs_bytes)
        .map_err(|e| format!("failed to parse bfbs '{}': {}", bfbs_path.display(), e))?;

    // Collect transitive include files from the reflection graph. Each
    // `SchemaFile.filename()` is prefixed with `//` (flatc's
    // bfbs-filenames root marker); strip it and resolve against
    // `schema_dir`. Emit rerun-if-changed for every entry; the root is in
    // there too but we already printed it above (cargo de-duplicates).
    let mut include_paths: Vec<PathBuf> = Vec::new();
    if let Some(files) = schema.fbs_files() {
        for f in files {
            let resolved = schema_dir.join(f.filename().trim_start_matches(BFBS_ROOT_PREFIX));
            println!("cargo:rerun-if-changed={}", resolved.display());
            if resolved != *schema_path {
                include_paths.push(resolved);
            }
        }
    }

    // Emit Rust wire-format code. Single-file schemas compile the root
    // directly. Multi-file schemas concatenate per-include outputs, because
    // flatc's `--rust` on a root with includes only emits the root file's
    // own decls (the includes' generated code expects `use crate::*` from
    // sibling `_generated.rs` files).
    if include_paths.is_empty() {
        flatc_rust::run(flatc_rust::Args {
            inputs: &[schema_path],
            out_dir,
            ..Default::default()
        })
        .map_err(|e| {
            format!(
                "failed to compile schema '{}': {}",
                schema_path.display(),
                e
            )
        })?;
    } else {
        let mut combined_generated = String::new();
        combined_generated
            .push_str("// automatically generated by flatbed_build - do not edit\n\n");
        combined_generated.push_str("use core::mem;\n");
        combined_generated.push_str("use core::cmp::Ordering;\n\n");
        combined_generated.push_str("extern crate flatbuffers;\n");
        combined_generated.push_str("use self::flatbuffers::{EndianScalar, Follow};\n\n");

        for include_path in &include_paths {
            flatc_rust::run(flatc_rust::Args {
                inputs: &[include_path.as_path()],
                out_dir,
                ..Default::default()
            })
            .map_err(|e| {
                format!(
                    "failed to compile included schema '{}': {}",
                    include_path.display(),
                    e
                )
            })?;

            let include_stem = include_path.file_stem().unwrap().to_str().unwrap();
            let generated_path = out_dir.join(format!("{}_generated.rs", include_stem));
            let generated_content = std::fs::read_to_string(&generated_path).map_err(|e| {
                format!(
                    "failed to read generated file '{}': {}",
                    generated_path.display(),
                    e
                )
            })?;

            let module_content = extract_module_content(&generated_content);
            combined_generated.push_str(&module_content);
            combined_generated.push('\n');
        }

        let combined_path = out_dir.join(format!("{}_generated.rs", stem));
        std::fs::write(&combined_path, combined_generated)?;
    }

    let (schemas_by_namespace, enums_by_namespace) = build_reflected_schema(&schema, schema_dir)?;

    let flatbed_code = generate_flatbed_module(&schemas_by_namespace, &enums_by_namespace, stem);
    let flatbed_path = out_dir.join(format!("{}_flatbed.rs", stem));
    std::fs::write(&flatbed_path, flatbed_code)?;

    Ok(())
}

/// Extract module content from flatc-generated code (skip header imports).
///
/// flatc's `--rust` output starts with a shared preamble (`use core::mem;`,
/// extern crate, etc.) followed by `pub mod <namespace> { ... }`. When
/// concatenating multiple include outputs into one file, we keep one shared
/// preamble at the top and pull just the module blocks from each include.
fn extract_module_content(generated: &str) -> String {
    let mut in_module = false;
    let mut brace_depth = 0;
    let mut result = String::new();

    for line in generated.lines() {
        if line.starts_with("#[allow(") && line.contains("pub mod") {
            in_module = true;
        }
        if line.starts_with("pub mod ") {
            in_module = true;
        }

        if in_module {
            result.push_str(line);
            result.push('\n');

            // Track brace depth to know when module ends
            brace_depth += line.matches('{').count();
            brace_depth -= line.matches('}').count();

            if brace_depth == 0 && result.contains("pub mod") {
                break;
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compile_error_includes_schema_path() {
        let err = Config::new()
            .schema("nonexistent/missing.fbs")
            .out_dir("/tmp/flatbed_test_out")
            .compile()
            .unwrap_err();

        let msg = err.to_string();
        assert!(
            msg.contains("nonexistent/missing.fbs"),
            "error should contain schema path, got: {msg}"
        );
    }
}
