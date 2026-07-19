//! Verifies the recursive, `$ref`-based OpenAPI schema generation: every
//! generated type becomes a resolvable component (including tables reachable
//! only by nesting), enums render as string enums, vectors as arrays, scalars
//! carry integer formats, and each property carries its `x-fbs-*` extensions.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use generated::test::{TestRequest, UserRequest, UserResponse};

use flatbed::{route, FlatbedConfig, FlatbedRouteError, Request, Response};
use serde_json::Value;

#[route("/users", method = "POST", version = "v1", tag = "Users")]
async fn create_user(
    _req: Request<UserRequest>,
) -> Result<Response<UserResponse>, FlatbedRouteError> {
    Ok(Response::ok(UserResponse::default()))
}

/// A hand-written `ToFlatBuffer` type — never registered as a component, since
/// only `.fbs`-generated code submits to the type registry. A request body
/// must be a generated type (the macro calls its `from_flatbuffer`), but a
/// response type only needs `ToFlatBuffer`, so this exercises the non-generated
/// body path.
#[derive(Default, flatbed::serde::Serialize, flatbed::serde::Deserialize)]
#[serde(crate = "flatbed::serde")]
struct HandWritten {
    note: Option<String>,
}

impl flatbed::ToFlatBuffer for HandWritten {
    const SCHEMA_FIELDS: &'static [flatbed::FieldInfo] = &[flatbed::FieldInfo {
        name: "note",
        field_type: "string",
        fbs_type: "string",
        required: false,
    }];
    const SCHEMA_NAME: &'static str = "HandWritten";
    fn to_flatbuffer(&self) -> Vec<u8> {
        Vec::new()
    }
}

#[route("/notify", method = "POST", version = "v1")]
async fn notify(_req: Request<TestRequest>) -> Result<Response<HandWritten>, FlatbedRouteError> {
    Ok(Response::ok(HandWritten::default()))
}

fn spec() -> Value {
    let config = FlatbedConfig::new("Test API");
    let json = flatbed::get_openapi_json_for_version(&config, "v1");
    serde_json::from_str(&json).expect("spec is valid JSON")
}

fn collect_refs(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(map) => {
            for (k, val) in map {
                if k == "$ref" {
                    if let Some(s) = val.as_str() {
                        out.push(s.to_string());
                    }
                } else {
                    collect_refs(val, out);
                }
            }
        }
        Value::Array(arr) => arr.iter().for_each(|x| collect_refs(x, out)),
        _ => {}
    }
}

#[test]
fn nested_only_table_is_a_component() {
    let s = spec();
    let schemas = &s["components"]["schemas"];
    // Address is never a route body — it is only reachable by nesting inside
    // UserRequest/UserResponse/AddressBook — yet it must be a component so the
    // nested `$ref`s resolve.
    for name in [
        "Address",
        "UserRequest",
        "UserResponse",
        "AddressBook",
        "Severity",
    ] {
        assert!(schemas.get(name).is_some(), "missing component: {name}");
    }
}

#[test]
fn every_ref_resolves_to_a_component() {
    let s = spec();
    let schemas = s["components"]["schemas"]
        .as_object()
        .expect("components.schemas object");
    let mut refs = Vec::new();
    collect_refs(&s, &mut refs);
    assert!(!refs.is_empty(), "expected at least one $ref in the spec");
    for r in refs {
        let name = r
            .strip_prefix("#/components/schemas/")
            .unwrap_or_else(|| panic!("unexpected $ref shape: {r}"));
        assert!(schemas.contains_key(name), "unresolved $ref: {r}");
    }
}

#[test]
fn nested_table_field_is_allof_ref_with_extensions() {
    let s = spec();
    let address = &s["components"]["schemas"]["UserRequest"]["properties"]["address"];
    // A table reference wraps the $ref in allOf so the field can still carry
    // the x-fbs-* extensions alongside it.
    let all_of = address["allOf"].as_array().expect("address is allOf");
    assert_eq!(
        all_of[0]["$ref"].as_str(),
        Some("#/components/schemas/Address")
    );
    assert_eq!(address["x-fbs-type"].as_str(), Some("Address"));
    assert_eq!(address["x-fbs-id"].as_u64(), Some(2));
}

#[test]
fn scalar_field_carries_format_and_extensions() {
    let s = spec();
    // UserResponse.user_id is uint64 → integer / int64.
    let uid = &s["components"]["schemas"]["UserResponse"]["properties"]["user_id"];
    assert_eq!(uid["type"].as_str(), Some("integer"));
    assert_eq!(uid["format"].as_str(), Some("int64"));
    assert_eq!(uid["x-fbs-type"].as_str(), Some("uint64"));
    assert_eq!(uid["x-fbs-id"].as_u64(), Some(0));
}

#[test]
fn enum_is_string_component_with_variants_in_order() {
    let s = spec();
    let severity = &s["components"]["schemas"]["Severity"];
    assert_eq!(severity["type"].as_str(), Some("string"));
    assert_eq!(
        severity["enum"],
        serde_json::json!(["Info", "Warning", "Error"])
    );
}

#[test]
fn vector_field_is_array_of_ref_with_extensions() {
    let s = spec();
    let addresses = &s["components"]["schemas"]["AddressBook"]["properties"]["addresses"];
    assert_eq!(addresses["type"].as_str(), Some("array"));
    assert_eq!(
        addresses["items"]["$ref"].as_str(),
        Some("#/components/schemas/Address")
    );
    assert_eq!(addresses["x-fbs-type"].as_str(), Some("[Address]"));
}

#[test]
fn operation_body_references_component_and_keeps_both_content_types() {
    let s = spec();
    let post = &s["paths"]["/users"]["post"];
    assert_eq!(
        post["requestBody"]["content"]["application/json"]["schema"]["$ref"].as_str(),
        Some("#/components/schemas/UserRequest")
    );
    assert_eq!(
        post["responses"]["200"]["content"]["application/json"]["schema"]["$ref"].as_str(),
        Some("#/components/schemas/UserResponse")
    );
    // The FlatBuffer content entry is retained alongside JSON.
    assert!(post["requestBody"]["content"]["application/x-flatbuffers"].is_object());
    assert!(post["responses"]["200"]["content"]["application/x-flatbuffers"].is_object());
}

#[test]
fn unregistered_response_type_is_inlined_not_a_dangling_ref() {
    let s = spec();
    // HandWritten is not generated, so it is not a component...
    assert!(s["components"]["schemas"].get("HandWritten").is_none());
    // ...and its route inlines the schema rather than emitting a $ref that
    // nothing would resolve.
    let schema =
        &s["paths"]["/notify"]["post"]["responses"]["200"]["content"]["application/json"]["schema"];
    assert_eq!(schema["type"].as_str(), Some("object"));
    assert!(schema.get("$ref").is_none());
    assert!(schema["properties"]["note"].is_object());
}
