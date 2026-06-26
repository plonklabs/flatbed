//! Decode `flatc`'s binary schema (`.bfbs`) and present it to the codegen as
//! a small set of owned shadow types.
//!
//! The codegen consumes `Table` / `Field` / `Enum` view types bucketed by
//! namespace. We build them from the reflection schema: walk `objects()` and
//! `enums()`, strip the qualified-name prefix into a namespace bucket, and
//! map each reflected `Type` back to a small adapter language
//! (`"string"`, `"uint16"`, `"[Address]"`, `"Severity"`, …) that the
//! codegen mappers in `crate::fbs_types` understand.
//!
//! Source order recovery is handled by `crate::source_order` — the
//! reflection schema sorts entries alphabetically for binary search, but
//! the committed `*_flatbed.rs` snapshots were written in source order.

use flatbuffers_reflection::reflection::{BaseType, Schema, Type};
use std::collections::HashMap;
use std::path::Path;

use crate::source_order::source_decl_order;

/// flatc flags that affect the binary schema's content: include doc comments
/// and builtin attributes so the reflection graph carries them. Used by both
/// the production codegen path and the integration-style regression tests so
/// they always agree on what the bfbs contains.
pub(crate) const BFBS_FLATC_FLAGS: &[&str] = &["--bfbs-comments", "--bfbs-builtins"];

/// flatc's bfbs-filenames root marker. Every `SchemaFile.filename()`
/// returned by reflection is prefixed with this; strip it before resolving
/// the path against `schema_dir`.
pub(crate) const BFBS_ROOT_PREFIX: &str = "//";

/// Codegen view of a FlatBuffer table.
///
/// Built from the reflection graph (`schema.objects()`), one per non-struct
/// `Object`. Fields are listed in source order, recovered via `Field.id()`
/// (flatc assigns IDs sequentially in declaration order). The struct lives
/// here as an owned shadow type so the codegen consumers can take
/// `&[Table]` without holding a borrow on the bfbs bytes.
pub(crate) struct Table {
    pub(crate) name: String,
    pub(crate) fields: Vec<Field>,
}

/// Codegen view of a single table field.
///
/// `fbs_type` is the small adapter-language string the codegen mappers in
/// `crate::fbs_types` consume (`"string"`, `"uint16"`, `"[Address]"`,
/// `"Severity"`).
///
/// `default` carries the FB-declared default as a Rust literal expression
/// (`"8080"`, `"true"`, `"Severity::Warning"`, `"3.14_f32"`) when the
/// declared value differs from the field type's `Default::default()`.
/// `None` means "no override needed" — either no default was declared, or
/// the declared default equals the type default (`0` for scalars, the zero
/// variant for enums, `false` for bool). Carrying only non-trivial
/// defaults keeps generated `_flatbed.rs` snapshots byte-identical for
/// tables that don't use FB defaults.
pub(crate) struct Field {
    pub(crate) name: String,
    pub(crate) fbs_type: String,
    pub(crate) default: Option<String>,
}

/// Codegen view of a FlatBuffer enum.
///
/// `name` is the bare identifier (`Severity`, `BoxProtocol`); the namespace
/// prefix is stripped off when bucketing.
///
/// `variants` lists the variant identifiers ordered by their FlatBuffer
/// integer value. For schemas with the default implicit values (0, 1, 2, …)
/// this matches source order. For schemas with explicit values out of source
/// order, the variant-name serde adapters still round-trip correctly because
/// the adapter is keyed by variant name, not position.
pub(crate) struct Enum {
    pub(crate) name: String,
    pub(crate) variants: Vec<String>,
}

pub(crate) type TablesByNamespace = HashMap<String, Vec<Table>>;
pub(crate) type EnumsByNamespace = HashMap<String, Vec<Enum>>;

/// Walk the reflection graph and produce `(tables_by_namespace,
/// enums_by_namespace)` for the codegen.
///
/// Bucketing splits the qualified name (`v_1.PingRequest`) at the last `.`;
/// the prefix becomes the namespace key with `.` replaced by `::`, matching
/// the legacy single-file `namespace` directive interpretation. Within each
/// bucket, entries are sorted by `(declaration_file_index, source_position)`
/// to preserve source order across the include graph — necessary because
/// flatc sorts `Schema.objects()` and `Schema.enums()` alphabetically by
/// qualified name for binary-search purposes, but the committed
/// `*_flatbed.rs` snapshots were written in source order.
pub(crate) fn build_reflected_schema(
    schema: &Schema,
    schema_dir: &Path,
) -> Result<(TablesByNamespace, EnumsByNamespace), Box<dyn std::error::Error>> {
    // Per-file source-order index. We re-read each `.fbs` *only* to learn
    // the order in which top-level `table`/`enum` declarations appear —
    // not their types, not their fields. Type information all flows
    // through reflection.
    let mut file_order_index: HashMap<usize, HashMap<String, usize>> = HashMap::new();
    let mut file_index_by_name: HashMap<String, usize> = HashMap::new();
    if let Some(files) = schema.fbs_files() {
        for (i, f) in files.iter().enumerate() {
            let bfbs_name = f.filename().to_string();
            file_index_by_name.insert(bfbs_name.clone(), i);
            let resolved = schema_dir.join(bfbs_name.trim_start_matches(BFBS_ROOT_PREFIX));
            let text = std::fs::read_to_string(&resolved).map_err(|e| {
                format!(
                    "failed to read '{}' for source ordering: {}",
                    resolved.display(),
                    e
                )
            })?;
            let mut positions: HashMap<String, usize> = HashMap::new();
            for (pos, name) in source_decl_order(&text).into_iter().enumerate() {
                positions.entry(name).or_insert(pos);
            }
            file_order_index.insert(i, positions);
        }
    }

    type SortKey = (usize, usize);
    let mut tables_by_ns: HashMap<String, Vec<(Table, SortKey)>> = HashMap::new();
    let mut enums_by_ns: HashMap<String, Vec<(Enum, SortKey)>> = HashMap::new();

    for obj in schema.objects() {
        if obj.is_struct() {
            // FB `struct` (fixed-layout, scalars-only) is not supported by
            // the high-level codegen. Skip silently — flatc still emits
            // wire-format Rust for it, and no `_flatbed.rs` shadow is needed.
            continue;
        }
        let (ns, bare) = split_namespace(obj.name());

        let mut sorted_fields: Vec<_> = obj.fields().iter().collect();
        sorted_fields.sort_by_key(|f| f.id());
        let fields: Vec<Field> = sorted_fields
            .iter()
            .map(|f| {
                let ty = f.type_();
                Field {
                    name: f.name().to_string(),
                    fbs_type: reflection_type_to_fbs_string(&ty, schema),
                    default: reflected_field_default(f, &ty, schema),
                }
            })
            .collect();

        let key = sort_key(
            obj.declaration_file(),
            &bare,
            &file_index_by_name,
            &file_order_index,
        );
        tables_by_ns
            .entry(ns)
            .or_default()
            .push((Table { name: bare, fields }, key));
    }

    for en in schema.enums() {
        let (ns, bare) = split_namespace(en.name());
        let mut sorted_vals: Vec<_> = en.values().iter().collect();
        sorted_vals.sort_by_key(|v| v.value());
        let variants: Vec<String> = sorted_vals.iter().map(|v| v.name().to_string()).collect();
        let key = sort_key(
            en.declaration_file(),
            &bare,
            &file_index_by_name,
            &file_order_index,
        );
        enums_by_ns.entry(ns).or_default().push((
            Enum {
                name: bare,
                variants,
            },
            key,
        ));
    }

    let tables_sorted: TablesByNamespace = tables_by_ns
        .into_iter()
        .map(|(ns, mut entries)| {
            entries.sort_by_key(|(_, k)| *k);
            (ns, entries.into_iter().map(|(t, _)| t).collect())
        })
        .collect();
    let enums_sorted: EnumsByNamespace = enums_by_ns
        .into_iter()
        .map(|(ns, mut entries)| {
            entries.sort_by_key(|(_, k)| *k);
            (ns, entries.into_iter().map(|(e, _)| e).collect())
        })
        .collect();

    Ok((tables_sorted, enums_sorted))
}

fn sort_key(
    decl_file: Option<&str>,
    bare_name: &str,
    file_index_by_name: &HashMap<String, usize>,
    file_order_index: &HashMap<usize, HashMap<String, usize>>,
) -> (usize, usize) {
    let file_idx = decl_file
        .and_then(|df| file_index_by_name.get(df).copied())
        .unwrap_or(usize::MAX);
    let pos = file_order_index
        .get(&file_idx)
        .and_then(|m| m.get(bare_name).copied())
        .unwrap_or(usize::MAX);
    (file_idx, pos)
}

/// Split a fully-qualified FlatBuffer name into `(namespace, bare_name)`.
///
/// `v_1.PingRequest` → `("v_1", "PingRequest")`, `a.b.X` → `("a::b", "X")`,
/// `Loose` → `("", "Loose")` (no namespace declared).
fn split_namespace(qualified: &str) -> (String, String) {
    match qualified.rfind('.') {
        Some(i) => (
            qualified[..i].replace('.', "::"),
            qualified[i + 1..].to_string(),
        ),
        None => (String::new(), qualified.to_string()),
    }
}

fn bare_name(qualified: &str) -> &str {
    match qualified.rfind('.') {
        Some(i) => &qualified[i + 1..],
        None => qualified,
    }
}

/// Compute the FB-declared default for a field as a Rust literal expression.
///
/// Returns `None` when the declared default equals the field type's
/// `Default::default()` so the codegen can keep using `#[derive(Default)]`
/// for tables that don't carry any non-trivial defaults — this preserves
/// byte-identity for committed snapshots untouched by this feature.
///
/// FlatBuffer defaults are allowed only on scalars, bools, and enum-typed
/// scalar fields. flatc rejects defaults on strings, vectors, and tables
/// during schema parse, so we never see them here.
fn reflected_field_default(
    field: &flatbuffers_reflection::reflection::Field,
    ty: &Type,
    schema: &Schema,
) -> Option<String> {
    let bt = ty.base_type();
    if bt == BaseType::Vector || bt == BaseType::String || bt == BaseType::Obj {
        return None;
    }
    let idx = ty.index();

    // Enum-typed scalar field: `default_integer` holds the variant's value.
    // Look the variant up by value so the codegen emits `EnumName::Variant`.
    if idx >= 0 && is_integer_base(bt) {
        let dv = field.default_integer();
        if dv == 0 {
            // Zero variant — matches flatc-emitted `Default::default()`.
            return None;
        }
        let en = schema.enums().get(idx as usize);
        let bare_enum = bare_name(en.name());
        // flatc rejects schemas where a declared enum default doesn't match
        // an actual variant value, so this `find` is unreachable on any
        // bfbs flatc produced. Falling through silently (via `?`) would
        // emit `Default::default()` for the field — wrong generated code
        // with no build error. Panic instead to surface a corrupt bfbs
        // immediately, matching how `scalar_or_obj_to_fbs_string` handles
        // its own invariant violations.
        let variant = en
            .values()
            .iter()
            .find(|v| v.value() == dv)
            .unwrap_or_else(|| {
                panic!(
                    "flatc schema invariant violated: enum '{}' has no variant with value {}; \
                     the bfbs may be corrupt or produced by a non-standard tool",
                    en.name(),
                    dv
                )
            });
        return Some(format!("{}::{}", bare_enum, variant.name()));
    }

    match bt {
        BaseType::Bool => {
            if field.default_integer() != 0 {
                Some("true".to_string())
            } else {
                None
            }
        }
        BaseType::Byte
        | BaseType::UByte
        | BaseType::Short
        | BaseType::UShort
        | BaseType::Int
        | BaseType::UInt
        | BaseType::Long => {
            let dv = field.default_integer();
            if dv == 0 {
                None
            } else {
                Some(format!("{}", dv))
            }
        }
        BaseType::ULong => {
            let dv = field.default_integer();
            if dv == 0 {
                None
            } else {
                // `default_integer()` returns i64. flatc's own parser
                // clamps uint64 default literals to the i64 range (a
                // declared `uint64 = u64::MAX` silently lands as `0` in
                // the bfbs), so this cast is defensive against a
                // hand-crafted or non-standard bfbs whose stored value
                // has the high bit set — without the cast, that value
                // would sign-extend as a negative literal and the
                // emitted impl would fail to compile against the u64
                // field.
                Some(format!("{}", dv as u64))
            }
        }
        BaseType::Float => {
            let dv = field.default_real();
            if dv == 0.0 {
                None
            } else {
                // Cast to f32 first so the formatted literal carries the
                // field's storage precision — Rust's Display picks the
                // shortest decimal that round-trips back to the same f32,
                // which is the value any f32-typed reader of the wire
                // format would observe. The `_f32` suffix then pins the
                // literal type without an inference round.
                Some(format!("{}_f32", dv as f32))
            }
        }
        BaseType::Double => {
            let dv = field.default_real();
            if dv == 0.0 {
                None
            } else {
                Some(format!("{}_f64", dv))
            }
        }
        _ => None,
    }
}

/// Convert a reflected `Type` to the legacy `fbs_type` string shape the
/// codegen mappers expect (`"string"`, `"uint16"`, `"[Address]"`,
/// `"Severity"`, `"[Severity]"`).
///
/// Enum-typed scalar fields carry the underlying integer base type plus an
/// `index() >= 0` pointing into `schema.enums()`. We surface those as the
/// bare enum name so `fbs_type_to_rust_type` routes through the enum branch.
fn reflection_type_to_fbs_string(ty: &Type, schema: &Schema) -> String {
    let bt = ty.base_type();
    let idx = ty.index();
    if bt == BaseType::Vector {
        return format!(
            "[{}]",
            scalar_or_obj_to_fbs_string(ty.element(), idx, schema)
        );
    }
    scalar_or_obj_to_fbs_string(bt, idx, schema)
}

fn scalar_or_obj_to_fbs_string(bt: BaseType, idx: i32, schema: &Schema) -> String {
    // Integer base types with `idx >= 0` reference enums via
    // `schema.enums()[idx]`. The codegen treats enum-typed scalars distinctly
    // from raw integers, so surface them by bare name.
    if idx >= 0 && is_integer_base(bt) {
        let en = schema.enums().get(idx as usize);
        return bare_name(en.name()).to_string();
    }
    match bt {
        BaseType::Bool => "bool".to_string(),
        BaseType::Byte => "int8".to_string(),
        BaseType::UByte => "uint8".to_string(),
        BaseType::Short => "int16".to_string(),
        BaseType::UShort => "uint16".to_string(),
        BaseType::Int => "int32".to_string(),
        BaseType::UInt => "uint32".to_string(),
        BaseType::Long => "int64".to_string(),
        BaseType::ULong => "uint64".to_string(),
        BaseType::Float => "float32".to_string(),
        BaseType::Double => "float64".to_string(),
        BaseType::String => "string".to_string(),
        BaseType::Obj => {
            let obj = schema.objects().get(idx as usize);
            bare_name(obj.name()).to_string()
        }
        other => panic!(
            "unsupported FlatBuffer BaseType in flatbed_build codegen: {:?}. Unions, structs, arrays, and Vector64 are not yet supported by the high-level codegen.",
            other
        ),
    }
}

fn is_integer_base(bt: BaseType) -> bool {
    matches!(
        bt,
        BaseType::Byte
            | BaseType::UByte
            | BaseType::Short
            | BaseType::UShort
            | BaseType::Int
            | BaseType::UInt
            | BaseType::Long
            | BaseType::ULong
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use flatbuffers_reflection::reflection::root_as_schema;
    use std::path::PathBuf;

    #[test]
    fn test_split_namespace() {
        assert_eq!(
            split_namespace("app.data.PingRequest"),
            ("app::data".to_string(), "PingRequest".to_string())
        );
        assert_eq!(
            split_namespace("v_1.PingRequest"),
            ("v_1".to_string(), "PingRequest".to_string())
        );
        assert_eq!(
            split_namespace("Loose"),
            (String::new(), "Loose".to_string())
        );
    }

    // -- Regression tests exercising flatc + reflection end-to-end --
    //
    // These cases broke (or would have broken) the legacy line-oriented
    // parser. The reflection-driven flow inherits flatc's parsing — so
    // anything flatc accepts, the codegen accepts. These tests pin that
    // contract by feeding a tiny .fbs to flatc, decoding the resulting
    // .bfbs, and checking what `build_reflected_schema` derives.

    /// RAII guard around a per-test temp directory. Removes the directory
    /// on drop so a failing assertion doesn't leak files under `/tmp`.
    struct TempDir {
        path: PathBuf,
    }

    impl TempDir {
        fn new(label: &str) -> Self {
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!(
                "flatbed_build_test_{}_{}_{}",
                label,
                std::process::id(),
                nanos
            ));
            std::fs::create_dir_all(&path).expect("create tmp dir");
            Self { path }
        }

        fn path(&self) -> &Path {
            &self.path
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            // Best-effort cleanup. We don't unwrap because a panicking
            // test that aborts mid-write may leave partial state behind,
            // and there's no value in panicking-in-drop.
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    /// Write `content` to `<tmp_dir>/<name>` and return the path.
    fn write_temp_fbs(tmp_dir: &Path, name: &str, content: &str) -> PathBuf {
        let path = tmp_dir.join(name);
        std::fs::write(&path, content).expect("write tmp .fbs");
        path
    }

    /// Compile `root` with flatc `-b --schema` into `out_dir` and return
    /// the path of the root file's `.bfbs`.
    fn compile_bfbs(root: &Path, out_dir: &Path) -> PathBuf {
        let schema_dir = root.parent().unwrap();
        let schema_dir_str = schema_dir.to_str().unwrap();
        let mut extra: Vec<&str> = BFBS_FLATC_FLAGS.to_vec();
        extra.extend(["--bfbs-filenames", schema_dir_str]);
        flatc_rust::run(flatc_rust::Args {
            inputs: &[root],
            out_dir,
            binary: true,
            schema: true,
            extra: &extra,
            ..Default::default()
        })
        .expect("flatc -b --schema");
        out_dir.join(format!(
            "{}.bfbs",
            root.file_stem().unwrap().to_str().unwrap()
        ))
    }

    #[test]
    fn test_regression_trailing_line_comment_after_enum_variant() {
        // The legacy parser's variant extractor split on `,` and stripped
        // `= N` suffixes, but didn't strip `// ...` line comments — so a
        // trailing comment after a variant produced a spurious variant
        // named `"// my comment"`. Reflection feeds the codegen via flatc,
        // which strips comments before parsing — verified end-to-end.
        let tmp = TempDir::new("trailing_line_comment");
        let root = write_temp_fbs(
            tmp.path(),
            "trailing.fbs",
            r#"
namespace t;

enum Severity : byte {
  Info,      // info level
  Warning,   // warning level
  Error,     // error level
}

table Msg { sev: Severity; }
"#,
        );
        let bfbs = compile_bfbs(&root, tmp.path());
        let bytes = std::fs::read(&bfbs).unwrap();
        let schema = root_as_schema(&bytes).unwrap();
        let (_, enums_by_ns) = build_reflected_schema(&schema, tmp.path()).unwrap();
        let enums = enums_by_ns.get("t").expect("namespace 't' present");
        assert_eq!(enums.len(), 1);
        assert_eq!(enums[0].name, "Severity");
        assert_eq!(enums[0].variants, vec!["Info", "Warning", "Error"]);
    }

    #[test]
    fn test_regression_inline_block_comment_in_field_decl() {
        // The legacy parser worked off `line.split(':')` and string
        // prefixes, so a `/* … */` mid-line could shift tokens or corrupt
        // the captured type. flatc strips block comments before producing
        // reflection, so the codegen sees the clean type string.
        let tmp = TempDir::new("inline_block_comment");
        let root = write_temp_fbs(
            tmp.path(),
            "inline.fbs",
            r#"
namespace t;
table Thing {
  /* note */ name: /* a */ string;
  value: /* width */ uint32 /* trailing */;
}
"#,
        );
        let bfbs = compile_bfbs(&root, tmp.path());
        let bytes = std::fs::read(&bfbs).unwrap();
        let schema = root_as_schema(&bytes).unwrap();
        let (tables_by_ns, _) = build_reflected_schema(&schema, tmp.path()).unwrap();
        let tables = tables_by_ns.get("t").expect("namespace 't' present");
        let thing = tables
            .iter()
            .find(|t| t.name == "Thing")
            .expect("Thing table");
        let name_field = thing.fields.iter().find(|f| f.name == "name").unwrap();
        let value_field = thing.fields.iter().find(|f| f.name == "value").unwrap();
        assert_eq!(name_field.fbs_type, "string");
        assert_eq!(value_field.fbs_type, "uint32");
    }

    #[test]
    fn test_regression_cross_namespace_adapter_buckets_and_strips() {
        // The legacy text parser tripped at flatc-parse time on any field
        // declared as `other_ns.Type`. Reflection accepts the qualified
        // form; the adapter buckets the referenced type under its own
        // namespace and strips the prefix on the field-type string so
        // the codegen's single-namespace table lookup sees a bare name.
        // This test pins the two adapter properties; full
        // cross-namespace codegen (emitting `super::other_ns::Type` for
        // the field type) is not yet implemented.
        let tmp = TempDir::new("cross_namespace");
        write_temp_fbs(
            tmp.path(),
            "other.fbs",
            r#"
namespace other;
table Inner { x: int32; }
"#,
        );
        let root = write_temp_fbs(
            tmp.path(),
            "root.fbs",
            r#"
include "other.fbs";
namespace root;
table Outer { ref: other.Inner; }
"#,
        );
        let bfbs = compile_bfbs(&root, tmp.path());
        let bytes = std::fs::read(&bfbs).unwrap();
        let schema = root_as_schema(&bytes).unwrap();
        let (tables_by_ns, _) = build_reflected_schema(&schema, tmp.path()).unwrap();
        let root_tables = tables_by_ns.get("root").expect("namespace 'root' present");
        let outer = root_tables
            .iter()
            .find(|t| t.name == "Outer")
            .expect("Outer table");
        let ref_field = outer.fields.iter().find(|f| f.name == "ref").unwrap();
        // Adapter strips the `other.` prefix so the codegen sees a bare
        // name. The single-namespace lookup in `fbs_type_to_rust_type`
        // then routes through the table branch.
        assert_eq!(ref_field.fbs_type, "Inner");
        // And the cross-namespace table itself is bucketed under `other`.
        let other_tables = tables_by_ns
            .get("other")
            .expect("namespace 'other' present");
        assert!(other_tables.iter().any(|t| t.name == "Inner"));
    }

    #[test]
    fn test_reflection_type_adapter_all_basetypes() {
        // Pin the BaseType → fbs_type string mapping. Each scalar should
        // produce the explicit-width spelling (`int8`, `int32`, …) that
        // matches the codegen's `fbs_type_to_rust_serde` lookup; vectors
        // wrap the inner string in `[…]`; enum-typed scalars surface as
        // the bare enum name (not the underlying integer base).
        let tmp = TempDir::new("all_basetypes");
        let root = write_temp_fbs(
            tmp.path(),
            "types.fbs",
            r#"
namespace t;
enum Color : byte { Red, Green, Blue }
table Inner { v: int32; }
table All {
  f_bool: bool;
  f_i8: int8;
  f_u8: uint8;
  f_i16: int16;
  f_u16: uint16;
  f_i32: int32;
  f_u32: uint32;
  f_i64: int64;
  f_u64: uint64;
  f_f32: float32;
  f_f64: float64;
  f_string: string;
  f_obj: Inner;
  f_enum: Color;
  f_vec_i32: [int32];
  f_vec_string: [string];
  f_vec_obj: [Inner];
  f_vec_enum: [Color];
}
"#,
        );
        let bfbs = compile_bfbs(&root, tmp.path());
        let bytes = std::fs::read(&bfbs).unwrap();
        let schema = root_as_schema(&bytes).unwrap();
        let (tables_by_ns, _) = build_reflected_schema(&schema, tmp.path()).unwrap();
        let all = tables_by_ns
            .get("t")
            .unwrap()
            .iter()
            .find(|t| t.name == "All")
            .expect("All table");
        let by_name: HashMap<&str, &str> = all
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.fbs_type.as_str()))
            .collect();
        assert_eq!(by_name["f_bool"], "bool");
        assert_eq!(by_name["f_i8"], "int8");
        assert_eq!(by_name["f_u8"], "uint8");
        assert_eq!(by_name["f_i16"], "int16");
        assert_eq!(by_name["f_u16"], "uint16");
        assert_eq!(by_name["f_i32"], "int32");
        assert_eq!(by_name["f_u32"], "uint32");
        assert_eq!(by_name["f_i64"], "int64");
        assert_eq!(by_name["f_u64"], "uint64");
        assert_eq!(by_name["f_f32"], "float32");
        assert_eq!(by_name["f_f64"], "float64");
        assert_eq!(by_name["f_string"], "string");
        assert_eq!(by_name["f_obj"], "Inner");
        assert_eq!(by_name["f_enum"], "Color");
        assert_eq!(by_name["f_vec_i32"], "[int32]");
        assert_eq!(by_name["f_vec_string"], "[string]");
        assert_eq!(by_name["f_vec_obj"], "[Inner]");
        assert_eq!(by_name["f_vec_enum"], "[Color]");
    }

    #[test]
    fn test_reflected_field_default_extraction() {
        // Pin the BaseType → default-expression mapping. Each non-zero
        // FB-declared default produces a Rust literal the codegen pastes
        // verbatim into the custom `impl Default`. Zero / no-default
        // fields stay `None` so the table can keep `#[derive(Default)]`
        // and the snapshot stays byte-identical.
        let tmp = TempDir::new("field_defaults");
        let root = write_temp_fbs(
            tmp.path(),
            "defaults.fbs",
            r#"
namespace t;
enum Severity : byte { Info, Warning, Error }
table Defaults {
  port: uint16 = 8080;
  retries: int32;
  enabled: bool = true;
  disabled: bool = false;
  ratio: float32 = 0.5;
  negative_ratio: float32 = -0.5;
  precision: float64 = 0.0;
  level: Severity = Warning;
  default_level: Severity;
  zero_level: Severity = Info;
  signed_offset: int32 = -42;
  huge_id: uint64 = 9223372036854775807;
}
"#,
        );
        let bfbs = compile_bfbs(&root, tmp.path());
        let bytes = std::fs::read(&bfbs).unwrap();
        let schema = root_as_schema(&bytes).unwrap();
        let (tables_by_ns, _) = build_reflected_schema(&schema, tmp.path()).unwrap();
        let defaults = tables_by_ns
            .get("t")
            .unwrap()
            .iter()
            .find(|t| t.name == "Defaults")
            .expect("Defaults table");
        let by_name: HashMap<&str, Option<&str>> = defaults
            .fields
            .iter()
            .map(|f| (f.name.as_str(), f.default.as_deref()))
            .collect();
        // Non-zero scalar default — pass through as the literal value.
        assert_eq!(by_name["port"], Some("8080"));
        // Zero / no-default scalar — None means the codegen falls back to
        // `Default::default()` and keeps the struct's `derive(Default)`.
        assert_eq!(by_name["retries"], None);
        // Bool: only `true` needs an override; `false` matches the type
        // default and stays None.
        assert_eq!(by_name["enabled"], Some("true"));
        assert_eq!(by_name["disabled"], None);
        // Float defaults pin their literal type so the impl compiles
        // without an inference round.
        assert_eq!(by_name["ratio"], Some("0.5_f32"));
        // Negative float defaults flow through Rust's Display unchanged.
        assert_eq!(by_name["negative_ratio"], Some("-0.5_f32"));
        // Float-zero is the type default — no override.
        assert_eq!(by_name["precision"], None);
        // Negative signed integer defaults format with the leading `-`.
        assert_eq!(by_name["signed_offset"], Some("-42"));
        // Largest uint64 default flatc preserves through the bfbs is
        // `i64::MAX` — anything above that gets clamped to 0 by flatc's
        // own parser. The ULong arm's u64 cast is still defensive
        // (covers a hand-crafted bfbs whose stored value has the high
        // bit set), but the test pins the round-trip flatc actually
        // produces.
        assert_eq!(by_name["huge_id"], Some("9223372036854775807"));
        // Enum default resolved by `default_integer`: variant value 1 →
        // `Severity::Warning`.
        assert_eq!(by_name["level"], Some("Severity::Warning"));
        // Enum field with no declared default lands on the zero variant
        // via `Severity::default()` — stays None.
        assert_eq!(by_name["default_level"], None);
        // Enum default explicitly set to the zero variant is
        // indistinguishable in reflection from no-default, and the Rust
        // behaviour is the same either way — stays None.
        assert_eq!(by_name["zero_level"], None);
    }
}
