//! Map FlatBuffer schema type strings to Rust types and OpenAPI shapes.
//!
//! The codegen consumes a small adapter language for type info — string
//! shapes like `"string"`, `"uint16"`, `"[Address]"`, `"Severity"`. The
//! reflection adapter (`crate::reflection`) produces these strings from
//! flatc's bfbs output; this module turns them into Rust type strings the
//! emitter writes verbatim into `_flatbed.rs`.

/// Convert FlatBuffer type to Rust serde type (for primitive types).
///
/// Returns `""` for non-primitive types (single tables, table vectors, enums,
/// enum vectors, or unknown types). The caller (`fbs_type_to_rust_type`)
/// resolves those by consulting the table- and enum-name lists.
pub(crate) fn fbs_type_to_rust_serde(fbs_type: &str) -> &'static str {
    if is_vector_type(fbs_type) {
        let inner = vector_inner_type(fbs_type);
        return match inner {
            "string" => "Option<Vec<String>>",
            "bool" => "Option<Vec<bool>>",
            "byte" | "int8" => "Option<Vec<i8>>",
            "ubyte" | "uint8" => "Option<Vec<u8>>",
            "short" | "int16" => "Option<Vec<i16>>",
            "ushort" | "uint16" => "Option<Vec<u16>>",
            "int" | "int32" => "Option<Vec<i32>>",
            "uint" | "uint32" => "Option<Vec<u32>>",
            "long" | "int64" => "Option<Vec<i64>>",
            "ulong" | "uint64" => "Option<Vec<u64>>",
            "float" | "float32" => "Option<Vec<f32>>",
            "double" | "float64" => "Option<Vec<f64>>",
            // Vector of tables (or unknown) — let fbs_type_to_rust_type resolve
            // it once it has the table-name list.
            _ => "",
        };
    }
    match fbs_type {
        "string" => "Option<String>",
        "bool" => "bool",
        "byte" | "int8" => "i8",
        "ubyte" | "uint8" => "u8",
        "short" | "int16" => "i16",
        "ushort" | "uint16" => "u16",
        "int" | "int32" => "i32",
        "uint" | "uint32" => "u32",
        "long" | "int64" => "i64",
        "ulong" | "uint64" => "u64",
        "float" | "float32" => "f32",
        "double" | "float64" => "f64",
        _ => "", // Return empty for non-primitive types (tables)
    }
}

/// Convert FlatBuffer type to Rust type, considering known table and enum types.
///
/// Resolution order:
/// 1. Primitive scalar / `Option<String>` / `Option<Vec<scalar>>` — handled by
///    `fbs_type_to_rust_serde`.
/// 2. `[Table]` → `Option<Vec<Table>>`.
/// 3. `[Enum]` → `Option<Vec<Enum>>` (enums are wire-level scalars, so a vector
///    of them is built and decoded with the same `create_vector` /
///    `iter().collect()` paths as a vector of primitives).
/// 4. Single table → `Option<Table>`.
/// 5. Bare enum → `Enum` (no `Option` — FlatBuffer enums are scalars and
///    default to the first variant when absent).
/// 6. Anything else is a schema bug → panic at codegen time.
pub(crate) fn fbs_type_to_rust_type(
    fbs_type: &str,
    table_names: &[String],
    enum_names: &[String],
) -> String {
    let primitive = fbs_type_to_rust_serde(fbs_type);
    if !primitive.is_empty() {
        return primitive.to_string();
    }

    // Vectors of scalars and strings are already handled by
    // `fbs_type_to_rust_serde` above. The only valid forms left are
    // `[KnownTable]` and `[KnownEnum]`. Anything else (typo, forward-reference,
    // wrong namespace) is a schema bug — fail loud at codegen time rather than
    // emit a wrong type and surprise the user with a Rust compile error far
    // from the .fbs file.
    if is_vector_type(fbs_type) {
        let inner = vector_inner_type(fbs_type);
        if table_names.iter().any(|t| t == inner) {
            return format!("Option<Vec<{}>>", inner);
        }
        if enum_names.iter().any(|e| e == inner) {
            return format!("Option<Vec<{}>>", inner);
        }
        panic!(
            "unsupported vector element type '[{inner}]' — not a known scalar, string, table, or enum"
        );
    }

    // Single nested table → Option<TableName>
    if table_names.iter().any(|t| t == fbs_type) {
        return format!("Option<{}>", fbs_type);
    }

    // Bare enum → EnumName (no Option — enums are scalars).
    if enum_names.iter().any(|e| e == fbs_type) {
        return fbs_type.to_string();
    }

    // Unknown type — fail loud at codegen so a typo or missing namespace
    // import in the .fbs file surfaces here, with the .fbs filename in the
    // build script's stack trace, instead of producing a wrong field type
    // (`Option<String>`) and surfacing as a confusing rustc error far from
    // the schema. Mirrors the vector-inner panic for the same reason.
    panic!(
        "unknown field type '{fbs_type}' — not a known scalar, string, table, or enum. Check for a typo or a missing namespace import."
    )
}

/// Check if a FlatBuffer type is a vector type (e.g. `[string]`)
pub(crate) fn is_vector_type(fbs_type: &str) -> bool {
    fbs_type.starts_with('[') && fbs_type.ends_with(']')
}

/// Extract the inner type from a vector type (e.g. `[string]` → `"string"`)
pub(crate) fn vector_inner_type(fbs_type: &str) -> &str {
    fbs_type.trim_start_matches('[').trim_end_matches(']')
}

/// Check if a FlatBuffer type is a single table reference (e.g. `Address`).
pub(crate) fn is_table_type(fbs_type: &str, table_names: &[String]) -> bool {
    let primitive = fbs_type_to_rust_serde(fbs_type);
    primitive.is_empty() && !is_vector_type(fbs_type) && table_names.iter().any(|t| t == fbs_type)
}

/// Check if a FlatBuffer type is a vector of a known table (e.g. `[Address]`).
pub(crate) fn is_table_vector(fbs_type: &str, table_names: &[String]) -> bool {
    if !is_vector_type(fbs_type) {
        return false;
    }
    let inner = vector_inner_type(fbs_type);
    table_names.iter().any(|t| t == inner)
}

/// Check if a FlatBuffer type is a single enum reference (e.g. `Severity`).
///
/// Enums are wire-level scalars (fixed-width integers) so they take the same
/// encode/decode path as primitives — no offset, no `Option`. The predicate
/// is consulted at codegen time to attach a `#[schema(value_type = String)]`
/// override to the field (flatc-emitted enums do not implement utoipa's
/// `ComposeSchema`, so the derive on the parent struct would otherwise fail).
pub(crate) fn is_enum_type(fbs_type: &str, enum_names: &[String]) -> bool {
    let primitive = fbs_type_to_rust_serde(fbs_type);
    primitive.is_empty() && !is_vector_type(fbs_type) && enum_names.iter().any(|e| e == fbs_type)
}

/// Check if a FlatBuffer type is a vector of a known enum (e.g. `[Severity]`).
///
/// `[Enum]` round-trips through the existing scalar-vector codegen because
/// flatc-emitted enums implement `Push + Copy`. As with single enum fields,
/// the predicate is consulted to attach a `#[schema(value_type = Vec<String>)]`
/// override on the surrounding struct.
pub(crate) fn is_enum_vector(fbs_type: &str, enum_names: &[String]) -> bool {
    if !is_vector_type(fbs_type) {
        return false;
    }
    let inner = vector_inner_type(fbs_type);
    enum_names.iter().any(|e| e == inner)
}

/// Convert FlatBuffer type to OpenAPI schema type
pub(crate) fn fbs_type_to_openapi(fbs_type: &str) -> &'static str {
    if is_vector_type(fbs_type) {
        return "array";
    }
    match fbs_type {
        "string" => "string",
        "bool" => "boolean",
        "byte" | "int8" | "ubyte" | "uint8" | "short" | "int16" | "ushort" | "uint16" | "int"
        | "int32" | "uint" | "uint32" | "long" | "int64" | "ulong" | "uint64" => "integer",
        "float" | "float32" | "double" | "float64" => "number",
        _ => "string", // Default fallback
    }
}

/// Determine if a FlatBuffer type maps to an optional Rust type
pub(crate) fn is_optional_type(fbs_type: &str) -> bool {
    // In FlatBuffers, strings and vector reference types are optional by default
    matches!(fbs_type, "string") || is_vector_type(fbs_type)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fbs_type_to_rust_serde() {
        assert_eq!(fbs_type_to_rust_serde("string"), "Option<String>");
        assert_eq!(fbs_type_to_rust_serde("bool"), "bool");
        assert_eq!(fbs_type_to_rust_serde("long"), "i64");
        assert_eq!(fbs_type_to_rust_serde("int"), "i32");
    }

    #[test]
    fn test_fbs_type_to_openapi() {
        assert_eq!(fbs_type_to_openapi("string"), "string");
        assert_eq!(fbs_type_to_openapi("bool"), "boolean");
        assert_eq!(fbs_type_to_openapi("long"), "integer");
        assert_eq!(fbs_type_to_openapi("float"), "number");
    }

    #[test]
    fn test_nested_table_detection() {
        let table_names = vec!["Address".to_string(), "User".to_string()];

        // Address should be detected as a table type
        assert!(is_table_type("Address", &table_names));
        assert!(is_table_type("User", &table_names));

        // Primitive types should not be detected as tables
        assert!(!is_table_type("string", &table_names));
        assert!(!is_table_type("int", &table_names));
        assert!(!is_table_type("bool", &table_names));
    }

    #[test]
    fn test_fbs_type_to_rust_type_with_tables() {
        let table_names = vec!["Address".to_string()];
        let enum_names: Vec<String> = vec![];

        assert_eq!(
            fbs_type_to_rust_type("string", &table_names, &enum_names),
            "Option<String>"
        );
        assert_eq!(
            fbs_type_to_rust_type("int", &table_names, &enum_names),
            "i32"
        );
        assert_eq!(
            fbs_type_to_rust_type("Address", &table_names, &enum_names),
            "Option<Address>"
        );
    }

    #[test]
    fn test_is_vector_type() {
        assert!(is_vector_type("[string]"));
        assert!(is_vector_type("[int32]"));
        assert!(!is_vector_type("string"));
        assert!(!is_vector_type("int32"));
    }

    #[test]
    fn test_vector_inner_type() {
        assert_eq!(vector_inner_type("[string]"), "string");
        assert_eq!(vector_inner_type("[int32]"), "int32");
    }

    #[test]
    fn test_fbs_type_to_rust_serde_vector() {
        assert_eq!(fbs_type_to_rust_serde("[string]"), "Option<Vec<String>>");
        assert_eq!(fbs_type_to_rust_serde("[int32]"), "Option<Vec<i32>>");
        assert_eq!(fbs_type_to_rust_serde("[float]"), "Option<Vec<f32>>");
        assert_eq!(fbs_type_to_rust_serde("[bool]"), "Option<Vec<bool>>");
    }

    #[test]
    fn test_fbs_type_to_rust_serde_table_vector_passes_through() {
        // Table vectors return "" so the caller (fbs_type_to_rust_type) can
        // resolve them with the table-name list.
        assert_eq!(fbs_type_to_rust_serde("[SomeTable]"), "");
    }

    #[test]
    fn test_fbs_type_to_rust_type_table_vector() {
        let tables = vec!["Address".to_string()];
        let enums: Vec<String> = vec![];
        assert_eq!(
            fbs_type_to_rust_type("[Address]", &tables, &enums),
            "Option<Vec<Address>>"
        );
    }

    #[test]
    #[should_panic(expected = "unsupported vector element type '[Unknown]'")]
    fn test_fbs_type_to_rust_type_unknown_vector_inner_panics() {
        // Unknown inner — neither a scalar, table, nor enum — must fail
        // loud at codegen time. Otherwise a typo in a `.fbs` schema would
        // produce a wrong field type and surface as a confusing Rust
        // compile error.
        let tables = vec!["Address".to_string()];
        let enums = vec!["Severity".to_string()];
        fbs_type_to_rust_type("[Unknown]", &tables, &enums);
    }

    #[test]
    #[should_panic(expected = "unknown field type 'Severityy'")]
    fn test_fbs_type_to_rust_type_unknown_bare_field_panics() {
        // Symmetric to the vector-inner case: a misspelled bare-field type
        // must also fail loud at codegen time. Otherwise the schema's wrong
        // type would silently surface as `Option<String>` and only error at
        // Rust compile time, far from the .fbs file.
        let tables = vec!["Address".to_string()];
        let enums = vec!["Severity".to_string()];
        fbs_type_to_rust_type("Severityy", &tables, &enums);
    }

    #[test]
    fn test_is_table_vector() {
        let tables = vec!["Address".to_string(), "Port".to_string()];
        assert!(is_table_vector("[Address]", &tables));
        assert!(is_table_vector("[Port]", &tables));
        assert!(!is_table_vector("[string]", &tables));
        assert!(!is_table_vector("[Other]", &tables));
        assert!(!is_table_vector("Address", &tables));
    }

    #[test]
    fn test_is_table_type_excludes_vectors() {
        // is_table_type is for single nested tables only — `[Address]` is a
        // table vector, not a single-table reference.
        let tables = vec!["Address".to_string()];
        assert!(is_table_type("Address", &tables));
        assert!(!is_table_type("[Address]", &tables));
    }

    #[test]
    fn test_fbs_type_to_openapi_vector() {
        assert_eq!(fbs_type_to_openapi("[string]"), "array");
    }

    #[test]
    fn test_is_optional_type_vector() {
        assert!(is_optional_type("[string]"));
    }

    #[test]
    fn test_fbs_type_to_rust_type_returns_bare_enum_type() {
        let tables: Vec<String> = vec![];
        let enums = vec!["BoxProtocol".to_string(), "Severity".to_string()];

        // Bare enum field has no Option wrapper — FlatBuffer enums are
        // wire-level scalars and default to the first variant when absent.
        assert_eq!(
            fbs_type_to_rust_type("BoxProtocol", &tables, &enums),
            "BoxProtocol"
        );
        assert_eq!(
            fbs_type_to_rust_type("Severity", &tables, &enums),
            "Severity"
        );
    }

    #[test]
    fn test_fbs_type_to_rust_type_returns_vec_for_enum_vector() {
        let tables: Vec<String> = vec![];
        let enums = vec!["Severity".to_string()];

        // Vectors of enums behave like vectors of scalars at the wire level
        // (Push + Copy), but the surface API still wraps them in `Option<Vec>`
        // because the vector field itself is optional in FlatBuffers.
        assert_eq!(
            fbs_type_to_rust_type("[Severity]", &tables, &enums),
            "Option<Vec<Severity>>"
        );
    }

    #[test]
    fn test_is_enum_type() {
        let enums = vec!["Severity".to_string(), "Mode".to_string()];
        assert!(is_enum_type("Severity", &enums));
        assert!(is_enum_type("Mode", &enums));
        assert!(!is_enum_type("string", &enums));
        assert!(!is_enum_type("Other", &enums));
        // Vector form is not a single enum reference.
        assert!(!is_enum_type("[Severity]", &enums));
    }

    #[test]
    fn test_is_enum_vector() {
        let enums = vec!["Severity".to_string()];
        assert!(is_enum_vector("[Severity]", &enums));
        assert!(!is_enum_vector("[string]", &enums));
        assert!(!is_enum_vector("[Other]", &enums));
        assert!(!is_enum_vector("Severity", &enums));
    }

    #[test]
    #[should_panic(expected = "unsupported vector element type '[Unknown]'")]
    fn test_fbs_type_to_rust_type_unknown_vector_inner_panics_when_neither_table_nor_enum() {
        // Doubles as a regression guard: the panic must still fire even
        // when an enum list is present but doesn't contain the inner type.
        let tables = vec!["Address".to_string()];
        let enums = vec!["Severity".to_string()];
        fbs_type_to_rust_type("[Unknown]", &tables, &enums);
    }
}
