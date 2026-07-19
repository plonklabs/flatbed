//! Driver: configure schemas, invoke `flatc` twice per file (once for the
//! FlatBuffer wire-format Rust, once for the `.bfbs` reflection binary), and
//! hand the reflected graph to the codegen.

use flatbuffers_reflection::reflection::root_as_schema;
use std::path::{Path, PathBuf};

use crate::codegen::generate_flatbed_module;
use crate::reflection::{
    build_reflected_schema, EnumsByNamespace, TablesByNamespace, BFBS_FLATC_FLAGS, BFBS_ROOT_PREFIX,
};

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

/// The sorted top-level `.fbs` files in `dir`. Subdirectories are intentionally
/// ignored — the convention is that root files live in `<dir>/*.fbs` and pull
/// in versioned schemas via FlatBuffer `include` from subdirs like `v1/`.
/// Sorting gives deterministic output across re-runs.
pub fn root_fbs_files(dir: &Path) -> Result<Vec<PathBuf>, std::io::Error> {
    let mut roots: Vec<PathBuf> = std::fs::read_dir(dir)?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("fbs"))
        .collect();
    roots.sort();
    Ok(roots)
}

/// Emit a `.bfbs` for `schema_path` into `out_dir`, decode it, and return the
/// reflected `(tables, enums)` plus the resolved transitive `.fbs` files (the
/// root included).
///
/// `-b --schema` (`binary: true, schema: true`) emits a `.bfbs` covering the
/// full include graph in one file; `flatbuffers-reflection` decodes it, so the
/// codegen path never parses `.fbs` text.
pub(crate) fn reflect_schema_file(
    schema_path: &Path,
    out_dir: &Path,
) -> Result<(TablesByNamespace, EnumsByNamespace, Vec<PathBuf>), Box<dyn std::error::Error>> {
    let schema_dir = schema_path.parent().unwrap_or(Path::new("."));
    let stem = schema_path.file_stem().unwrap().to_str().unwrap();

    // Emit `.bfbs`. flatc follows includes and writes a single binary schema
    // covering the full graph; the bfbs's `fbs_files` list carries the
    // transitive includes.
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

    // Each `SchemaFile.filename()` is prefixed with `//` (flatc's
    // bfbs-filenames root marker); strip it and resolve against `schema_dir`.
    let mut include_files: Vec<PathBuf> = Vec::new();
    if let Some(files) = schema.fbs_files() {
        for f in files {
            include_files.push(schema_dir.join(f.filename().trim_start_matches(BFBS_ROOT_PREFIX)));
        }
    }

    let (tables, enums) = build_reflected_schema(&schema, schema_dir)?;
    Ok((tables, enums, include_files))
}

/// Compile a single schema into `_generated.rs` (FlatBuffer wire format) and
/// `_flatbed.rs` (plain structs + serde), printing `cargo:rerun-if-changed`
/// for every transitive `.fbs`. flatc's `--rust` on a root with `include`
/// directives only emits the root's own decls, so multi-file schemas are
/// concatenated from per-include runs.
fn compile_one_schema(
    schema_path: &Path,
    out_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let stem = schema_path.file_stem().unwrap().to_str().unwrap();

    println!("cargo:rerun-if-changed={}", schema_path.display());

    let (schemas_by_namespace, enums_by_namespace, include_files) =
        reflect_schema_file(schema_path, out_dir)?;

    // Emit rerun-if-changed for every transitive include; the root is in there
    // too but we already printed it above (cargo de-duplicates). Everything
    // but the root feeds the multi-file wire-format concatenation below.
    let mut include_paths: Vec<PathBuf> = Vec::new();
    for resolved in include_files {
        println!("cargo:rerun-if-changed={}", resolved.display());
        if resolved != *schema_path {
            include_paths.push(resolved);
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
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    /// Build a temp tree containing the given relative filenames as empty
    /// files. An atomic counter (process-id + monotone index) keeps parallel
    /// `cargo test` threads from colliding on the same directory.
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

    fn names(dir: &Path) -> Vec<String> {
        root_fbs_files(dir)
            .unwrap()
            .iter()
            .map(|p| p.file_name().unwrap().to_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn root_fbs_files_picks_up_top_level() {
        let dir = make_tree(&["operator.fbs", "user.fbs"]);
        assert_eq!(names(&dir), vec!["operator.fbs", "user.fbs"]);
    }

    #[test]
    fn root_fbs_files_ignores_subdirectories() {
        // Versioned schemas under `v1/` are reached via `include` from the
        // roots; compiling them as their own root would double up the output.
        let dir = make_tree(&["operator.fbs", "v1/operator.fbs", "v1/user.fbs"]);
        assert_eq!(names(&dir), vec!["operator.fbs"]);
    }

    #[test]
    fn root_fbs_files_ignores_non_fbs() {
        let dir = make_tree(&["operator.fbs", "README.md", "operator.fbs.bak"]);
        assert_eq!(names(&dir), vec!["operator.fbs"]);
    }

    #[test]
    fn root_fbs_files_returns_sorted() {
        let dir = make_tree(&["zzz.fbs", "aaa.fbs", "mmm.fbs"]);
        assert_eq!(names(&dir), vec!["aaa.fbs", "mmm.fbs", "zzz.fbs"]);
    }

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
