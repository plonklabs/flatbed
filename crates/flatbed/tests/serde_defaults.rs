//! A generated table with declared field defaults must deserialise a missing
//! field to its *declared* default — including an enum field whose default is a
//! non-zero variant, which container-level `#[serde(default)]` handles and
//! field-level `default` would get wrong (it would use the enum's zero variant).

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use generated::test::{Defaulted, Severity};

#[test]
fn absent_fields_deserialize_to_declared_defaults() {
    let d: Defaulted = flatbed::serde_json::from_str("{}").expect("empty object");
    assert_eq!(d.count, 25);
    assert!(d.flag);
    assert_eq!(d.ratio, 1.5);
    assert_eq!(d.level, Severity::Warning); // non-zero declared default, not Info
}

#[test]
fn present_field_overrides_default() {
    let d: Defaulted =
        flatbed::serde_json::from_str(r#"{"level":"Info","count":5}"#).expect("partial object");
    assert_eq!(d.level, Severity::Info);
    assert_eq!(d.count, 5);
    // untouched fields still fall back to their declared defaults
    assert!(d.flag);
    assert_eq!(d.ratio, 1.5);
}
