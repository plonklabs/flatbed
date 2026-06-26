//! End-to-end test for FlatBuffer enum codegen.
//!
//! Builds a `LogEvent` (which contains a bare `Severity` enum field plus an
//! `Option<Vec<Severity>>` history vector), encodes to FlatBuffer bytes,
//! decodes, and asserts equality. Exercises the codegen path added to
//! `flatbed_build` for `Enum` and `[Enum]` fields.
//!
//! Variant coverage is deliberately broader than the default: the FlatBuffer
//! default for an absent enum field is the first variant (`Severity::Info`),
//! so a round-trip that only ever uses `Info` would silently pass even if the
//! decoder dropped the value on the floor. We pin specific non-default
//! variants in the assertions to catch that class of regression.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use generated::test::{LogEvent, Severity};

#[test]
fn log_event_with_enum_field_round_trips() {
    let original = LogEvent {
        message: Some("disk full".to_string()),
        severity: Severity::Error,
        history: Some(vec![Severity::Info, Severity::Warning, Severity::Error]),
    };

    let bytes = original.to_flatbuffer();
    let decoded = LogEvent::from_flatbuffer(&bytes).expect("flatbuffer decode failed");

    assert_eq!(decoded, original);
    // Pin the specific non-default variants to guard against a decoder that
    // returns `Severity::Info` (the default) regardless of the encoded value.
    assert_eq!(decoded.severity, Severity::Error);
    assert_eq!(
        decoded.history.as_deref(),
        Some([Severity::Info, Severity::Warning, Severity::Error].as_slice()),
    );
}

#[test]
fn log_event_with_default_enum_round_trips() {
    // Even when `severity` matches the FlatBuffer default (`Severity::Info`),
    // the round-trip must preserve it. Tests the absent/default-equal scalar
    // path on the decoder side.
    let original = LogEvent {
        message: Some("ping".to_string()),
        severity: Severity::Info,
        history: None,
    };

    let bytes = original.to_flatbuffer();
    let decoded = LogEvent::from_flatbuffer(&bytes).expect("flatbuffer decode failed");

    assert_eq!(decoded, original);
    assert_eq!(decoded.severity, Severity::Info);
    assert!(decoded.history.is_none());
}

#[test]
fn log_event_with_empty_enum_vector_round_trips() {
    let original = LogEvent {
        message: None,
        severity: Severity::Warning,
        history: Some(vec![]),
    };

    let bytes = original.to_flatbuffer();
    let decoded = LogEvent::from_flatbuffer(&bytes).expect("flatbuffer decode failed");

    assert_eq!(decoded, original);
    assert_eq!(decoded.severity, Severity::Warning);
    assert_eq!(decoded.history.as_ref().map(Vec::len), Some(0));
}

#[test]
fn log_event_default_uses_first_enum_variant() {
    // `Severity` derives `Default` (flatc emits this on every enum), and the
    // default must match the first declared variant. This is the behaviour
    // FlatBuffer relies on for absent fields, and the codegen relies on it
    // for the bare-enum struct field having a sensible `Default` impl.
    assert_eq!(LogEvent::default().severity, Severity::Info);
    assert_eq!(Severity::default(), Severity::Info);
}

#[test]
fn log_event_json_omits_enum_field_uses_default() {
    // Mirrors the FlatBuffer "absent scalar = first variant" wire semantics
    // for JSON callers. Without `#[serde(default)]` on bare-enum fields, this
    // payload would be a hard deserialisation error instead of silently
    // taking `Severity::Info`.
    let json = r#"{ "message": "no severity in payload" }"#;
    let parsed: LogEvent = serde_json::from_str(json).expect("JSON decode failed");
    assert_eq!(parsed.severity, Severity::Info);
    assert_eq!(parsed.message.as_deref(), Some("no severity in payload"));
    assert!(parsed.history.is_none());
}

#[test]
fn log_event_json_serialises_enum_as_variant_name_string() {
    // Wire form must be the variant-name string ("Error"), not the underlying
    // integer (2). The OpenAPI schema declares `value_type = String`, and
    // any client generated from that spec expects a string here. Mismatches
    // would silently break round-trip with such clients.
    let event = LogEvent {
        message: Some("disk full".to_string()),
        severity: Severity::Error,
        history: Some(vec![Severity::Info, Severity::Warning, Severity::Error]),
    };
    let json = serde_json::to_value(&event).expect("JSON encode failed");
    assert_eq!(json["severity"], serde_json::Value::String("Error".into()));
    let history_json = json["history"].clone();
    assert_eq!(
        history_json,
        serde_json::json!(["Info", "Warning", "Error"])
    );

    // Round-trip back to the typed struct via the variant-name shape.
    let decoded: LogEvent = serde_json::from_value(json).expect("JSON decode failed");
    assert_eq!(decoded, event);
}

#[test]
fn log_event_json_rejects_unknown_variant_name() {
    // Unknown variant names must surface as a clear deserialisation error
    // listing the valid names — not a silent fallback to the first variant
    // and not a confusing parse error far from the field.
    let json = r#"{ "message": "x", "severity": "Catastrophic" }"#;
    let err = serde_json::from_str::<LogEvent>(json).expect_err("expected unknown-variant error");
    let msg = err.to_string();
    assert!(
        msg.contains("Catastrophic")
            && msg.contains("Info")
            && msg.contains("Warning")
            && msg.contains("Error"),
        "error should name the unknown variant and list valid ones: {msg}"
    );
}

#[test]
fn log_event_json_rejects_integer_for_enum_field() {
    // Integers are rejected — the wire format is a string. This guards
    // against accidentally regressing to the previous (integer-wire)
    // adapter and silently accepting both shapes during deserialisation.
    let json = r#"{ "message": "x", "severity": 2 }"#;
    let err = serde_json::from_str::<LogEvent>(json)
        .expect_err("integer should not deserialise as a string-typed enum");
    let msg = err.to_string();
    assert!(
        msg.contains("string") || msg.contains("integer") || msg.contains("expected"),
        "error should indicate type mismatch: {msg}"
    );
}
