//! Verifies the compile-time type registry populated by `TypeSchema` /
//! `EnumSchema` submissions in generated code, including tables reachable only
//! by nesting rather than as a route request/response.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use flatbed::{get_enum_schema, get_schema_bfbs, get_type_schema};

#[test]
fn schema_bfbs_is_baked_in() {
    let bfbs = get_schema_bfbs().expect("a compiled schema registers its .bfbs");
    // The runtime serves exactly the committed reflection byproduct.
    assert_eq!(bfbs, include_bytes!("../src/generated/test.bfbs") as &[u8]);
    assert!(bfbs.len() > 4, "a real reflection buffer is non-trivial");
}

#[test]
fn every_table_is_registered_including_nested_only() {
    for name in [
        "TestRequest",
        "TestResponse",
        "Address", // reachable only by nesting
        "UserRequest",
        "UserResponse",
        "AddressBook",
        "LogEvent",
    ] {
        let ty = get_type_schema(name).unwrap_or_else(|| panic!("type {name} not registered"));
        assert_eq!(ty.name, name);
        assert_eq!(ty.namespace, "test");
    }
}

#[test]
fn field_descriptors_carry_fbs_type_id_and_requiredness() {
    let ur = get_type_schema("UserRequest").expect("UserRequest not registered");

    // Field ids are the FlatBuffer vtable slots, sequential in declaration
    // order for these implicit-id schemas.
    let ids: Vec<u16> = ur.fields.iter().map(|f| f.field_id).collect();
    assert_eq!(ids, vec![0, 1, 2]);

    let address = ur
        .fields
        .iter()
        .find(|f| f.name == "address")
        .expect("address field missing");
    assert_eq!(address.fbs_type, "Address");
    assert!(!address.required);

    let age = ur.fields.iter().find(|f| f.name == "age").unwrap();
    assert_eq!(age.fbs_type, "int32");
    assert!(age.required);
}

#[test]
fn vector_and_scalar_fbs_types_are_preserved() {
    let book = get_type_schema("AddressBook").expect("AddressBook not registered");
    let addresses = book.fields.iter().find(|f| f.name == "addresses").unwrap();
    assert_eq!(addresses.fbs_type, "[Address]");
    assert!(!addresses.required);
}

#[test]
fn enums_are_registered_with_variants_in_value_order() {
    let sev = get_enum_schema("Severity").expect("Severity not registered");
    assert_eq!(sev.namespace, "test");
    assert_eq!(sev.variants, ["Info", "Warning", "Error"]);
}

#[test]
fn registry_has_no_bare_name_collisions() {
    flatbed::validate_type_registry().expect("test schema type/enum names are unique");
}
