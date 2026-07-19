//! `flatbed gen-fb-plugin`: cross-check a served OpenAPI spec against the local
//! `.fbs` schemas.
//!
//! A FlatBuffer client can only be generated for an operation whose
//! request/response types are present in the local schemas — the codec is built
//! from `.fbs` reflection, not from the spec. This module pulls the spec (from a
//! running server or a file), reflects the local schemas, and fails loudly when
//! an `application/x-flatbuffers` operation references a type that isn't there,
//! which means the local schemas are out of sync with the deployed server.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::compile::{reflect_schema_file, root_fbs_files};

/// Where to read the OpenAPI spec from.
pub enum SpecSource {
    /// Base URL of a running flatbed server; `/openapi.json` is appended.
    Server(String),
    /// A spec file on disk.
    File(PathBuf),
}

/// An operation that advertises the `application/x-flatbuffers` content type,
/// with the bare component names of its JSON request/response schemas (`None`
/// when the body is absent or inlined rather than referenced by `$ref`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct FbOperation {
    path: String,
    method: String,
    request_type: Option<String>,
    response_type: Option<String>,
}

/// Load the spec, reflect the local schemas, and validate that every
/// FlatBuffer operation's referenced types exist locally.
pub fn gen_fb_plugin(
    source: SpecSource,
    schemas_dir: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let spec = load_spec(source)?;
    let table_names = reflect_table_names(schemas_dir)?;
    let ops = fb_operations(&spec);
    validate(&ops, &table_names)?;
    println!(
        "gen-fb-plugin: validated {} FlatBuffer operation(s) against {} — all referenced types resolve.",
        ops.len(),
        schemas_dir.display(),
    );
    Ok(())
}

fn load_spec(source: SpecSource) -> Result<Value, Box<dyn std::error::Error>> {
    let text = match source {
        SpecSource::File(path) => std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read spec '{}': {e}", path.display()))?,
        SpecSource::Server(base) => {
            let url = format!("{}/openapi.json", base.trim_end_matches('/'));
            ureq::get(&url)
                .call()
                .map_err(|e| format!("failed to fetch spec from '{url}': {e}"))?
                .into_string()
                .map_err(|e| format!("failed to read spec body from '{url}': {e}"))?
        }
    };
    serde_json::from_str(&text).map_err(|e| format!("spec is not valid JSON: {e}").into())
}

/// The bare component name a `$ref` schema points at, e.g.
/// `{"$ref": "#/components/schemas/UserRequest"}` → `UserRequest`.
fn ref_type_name(schema: &Value) -> Option<String> {
    schema
        .get("$ref")?
        .as_str()?
        .strip_prefix("#/components/schemas/")
        .map(str::to_string)
}

fn json_ref(content: Option<&Value>) -> Option<String> {
    content
        .and_then(|c| c.get("application/json"))
        .and_then(|j| j.get("schema"))
        .and_then(ref_type_name)
}

fn advertises_flatbuffers(content: Option<&Value>) -> bool {
    content
        .and_then(|c| c.get("application/x-flatbuffers"))
        .is_some()
}

/// The `content` of the operation's first success (`2xx`) response. flatbed
/// emits `200`, but a spec may use another success code (`201`, `2XX`), so
/// hard-coding `200` would silently skip those and let validation pass over a
/// missing type.
fn success_response_content(op: &Value) -> Option<&Value> {
    op.get("responses")?
        .as_object()?
        .iter()
        .filter(|(code, _)| code.starts_with('2'))
        .find_map(|(_, resp)| resp.get("content"))
}

const HTTP_METHODS: &[&str] = &["get", "put", "post", "delete", "patch", "head", "options"];

fn fb_operations(spec: &Value) -> Vec<FbOperation> {
    let mut ops = Vec::new();
    let Some(paths) = spec.get("paths").and_then(Value::as_object) else {
        return ops;
    };
    for (path, item) in paths {
        let Some(item) = item.as_object() else {
            continue;
        };
        for (method, op) in item {
            if !HTTP_METHODS.contains(&method.as_str()) {
                continue;
            }
            let req_content = op.get("requestBody").and_then(|b| b.get("content"));
            let resp_content = success_response_content(op);
            if !advertises_flatbuffers(req_content) && !advertises_flatbuffers(resp_content) {
                continue;
            }
            ops.push(FbOperation {
                path: path.clone(),
                method: method.to_uppercase(),
                request_type: json_ref(req_content),
                response_type: json_ref(resp_content),
            });
        }
    }
    ops
}

fn validate(
    ops: &[FbOperation],
    table_names: &BTreeSet<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut missing: Vec<(String, String)> = Vec::new();
    let mut untyped: Vec<String> = Vec::new();
    for op in ops {
        let refs: Vec<&String> = [&op.request_type, &op.response_type]
            .into_iter()
            .flatten()
            .collect();
        // A FlatBuffer operation with no generated (`$ref`) request or response
        // type has nothing a codec could be built from, so flag it rather than
        // counting it as validated.
        if refs.is_empty() {
            untyped.push(format!("{} {}", op.method, op.path));
            continue;
        }
        for ty in refs {
            if !table_names.contains(ty) {
                missing.push((format!("{} {}", op.method, op.path), ty.clone()));
            }
        }
    }
    if missing.is_empty() && untyped.is_empty() {
        return Ok(());
    }
    let mut msg = String::new();
    if !missing.is_empty() {
        msg.push_str(
            "local .fbs schemas are out of sync with the deployed server — these types are \
             referenced by application/x-flatbuffers operations but were not found in the \
             schemas directory:\n",
        );
        for (op, ty) in &missing {
            msg.push_str(&format!("  - {op} needs type `{ty}`\n"));
        }
    }
    if !untyped.is_empty() {
        msg.push_str(
            "these application/x-flatbuffers operations reference no generated type, so no \
             FlatBuffer client can be built for them:\n",
        );
        for op in &untyped {
            msg.push_str(&format!("  - {op}\n"));
        }
    }
    // Trim so the caller's `eprintln!` doesn't leave a stray blank line.
    Err(msg.trim_end().to_string().into())
}

/// Reflect every top-level `.fbs` in `schemas_dir` (subdirectories are reached
/// via `include` directives) and collect the bare names of all tables.
fn reflect_table_names(schemas_dir: &Path) -> Result<BTreeSet<String>, Box<dyn std::error::Error>> {
    let scratch = std::env::temp_dir().join(format!("flatbed-fb-plugin-{}", std::process::id()));
    std::fs::create_dir_all(&scratch)?;
    let _guard = ScratchDir(scratch.clone());

    let roots = root_fbs_files(schemas_dir).map_err(|e| {
        format!(
            "failed to read schemas dir '{}': {e}",
            schemas_dir.display()
        )
    })?;
    if roots.is_empty() {
        return Err(format!("no .fbs files found in '{}'", schemas_dir.display()).into());
    }

    let mut names = BTreeSet::new();
    for root in roots {
        let (tables, _enums, _includes) = reflect_schema_file(&root, &scratch)?;
        for tables_in_ns in tables.values() {
            for table in tables_in_ns {
                names.insert(table.name.clone());
            }
        }
    }
    Ok(names)
}

struct ScratchDir(PathBuf);
impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec_json() -> Value {
        serde_json::json!({
            "paths": {
                "/users": { "post": {
                    "requestBody": { "content": {
                        "application/json": { "schema": { "$ref": "#/components/schemas/UserRequest" } },
                        "application/x-flatbuffers": { "schema": { "type": "string" } }
                    }},
                    "responses": { "200": { "content": {
                        "application/json": { "schema": { "$ref": "#/components/schemas/UserResponse" } },
                        "application/x-flatbuffers": { "schema": { "type": "string" } }
                    }}}
                }},
                "/health": { "get": {
                    "responses": { "200": { "content": {
                        "application/json": { "schema": { "$ref": "#/components/schemas/Health" } }
                    }}}
                }}
            }
        })
    }

    #[test]
    fn fb_operations_picks_only_flatbuffer_ops() {
        let ops = fb_operations(&spec_json());
        assert_eq!(ops.len(), 1);
        let op = &ops[0];
        assert_eq!(op.path, "/users");
        assert_eq!(op.method, "POST");
        assert_eq!(op.request_type.as_deref(), Some("UserRequest"));
        assert_eq!(op.response_type.as_deref(), Some("UserResponse"));
    }

    #[test]
    fn validate_passes_when_all_types_present() {
        let ops = fb_operations(&spec_json());
        let names: BTreeSet<String> = ["UserRequest", "UserResponse"]
            .into_iter()
            .map(String::from)
            .collect();
        assert!(validate(&ops, &names).is_ok());
    }

    #[test]
    fn success_response_under_non_200_code_is_still_seen() {
        let spec = serde_json::json!({
            "paths": { "/create": { "post": {
                "requestBody": { "content": {
                    "application/json": { "schema": { "$ref": "#/components/schemas/CreateReq" } },
                    "application/x-flatbuffers": { "schema": { "type": "string" } }
                }},
                "responses": { "201": { "content": {
                    "application/json": { "schema": { "$ref": "#/components/schemas/CreateResp" } },
                    "application/x-flatbuffers": { "schema": { "type": "string" } }
                }}}
            }}}
        });
        let ops = fb_operations(&spec);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].response_type.as_deref(), Some("CreateResp"));
    }

    #[test]
    fn validate_flags_flatbuffer_op_with_no_generated_type() {
        // Both bodies are inlined (no $ref), so nothing is codec-generatable.
        let spec = serde_json::json!({
            "paths": { "/opaque": { "post": {
                "requestBody": { "content": {
                    "application/json": { "schema": { "type": "object" } },
                    "application/x-flatbuffers": { "schema": { "type": "string" } }
                }},
                "responses": { "200": { "content": {
                    "application/json": { "schema": { "type": "object" } },
                    "application/x-flatbuffers": { "schema": { "type": "string" } }
                }}}
            }}}
        });
        let ops = fb_operations(&spec);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].request_type, None);
        assert_eq!(ops[0].response_type, None);
        let err = validate(&ops, &BTreeSet::new()).expect_err("untyped fb op must fail");
        let msg = err.to_string();
        assert!(msg.contains("no generated type"), "message: {msg}");
        assert!(msg.contains("POST /opaque"), "message: {msg}");
    }

    #[test]
    fn validate_reports_missing_type_with_operation() {
        let ops = fb_operations(&spec_json());
        let names: BTreeSet<String> = ["UserRequest"].into_iter().map(String::from).collect();
        let err = validate(&ops, &names).expect_err("missing type must fail");
        let msg = err.to_string();
        assert!(msg.contains("out of sync"), "message: {msg}");
        assert!(msg.contains("UserResponse"), "message: {msg}");
        assert!(msg.contains("POST /users"), "message: {msg}");
    }
}
