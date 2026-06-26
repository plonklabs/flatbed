//! End-to-end test for vector-of-tables FlatBuffer codegen.
//!
//! Builds an `AddressBook` (which contains `Vec<Address>` plus a `Vec<String>`),
//! encodes to FlatBuffer bytes, decodes, and asserts equality. Exercises the
//! codegen path added to `flatbed_build` for `[TableName]` fields.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use generated::test::{Address, AddressBook};

#[test]
fn address_book_round_trips_through_flatbuffer() {
    let original = AddressBook {
        owner: Some("Alice".to_string()),
        addresses: Some(vec![
            Address {
                street: Some("1 Infinite Loop".to_string()),
                city: Some("Cupertino".to_string()),
                zip_code: 95014,
            },
            Address {
                street: Some("350 5th Ave".to_string()),
                city: Some("New York".to_string()),
                zip_code: 10118,
            },
        ]),
        contact_names: Some(vec!["Bob".to_string(), "Carol".to_string()]),
    };

    let bytes = original.to_flatbuffer();
    let decoded = AddressBook::from_flatbuffer(&bytes).expect("flatbuffer decode failed");

    assert_eq!(decoded, original);
}

#[test]
fn address_book_with_empty_vectors_round_trips() {
    let original = AddressBook {
        owner: Some("nobody".to_string()),
        addresses: Some(vec![]),
        contact_names: Some(vec![]),
    };

    let bytes = original.to_flatbuffer();
    let decoded = AddressBook::from_flatbuffer(&bytes).expect("flatbuffer decode failed");

    assert_eq!(decoded, original);
}

#[test]
fn address_book_with_none_vectors_round_trips() {
    let original = AddressBook {
        owner: None,
        addresses: None,
        contact_names: None,
    };

    let bytes = original.to_flatbuffer();
    let decoded = AddressBook::from_flatbuffer(&bytes).expect("flatbuffer decode failed");

    assert_eq!(decoded, original);
}
