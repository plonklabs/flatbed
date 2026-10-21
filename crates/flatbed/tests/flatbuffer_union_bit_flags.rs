//! End-to-end test for union and `bit_flags` codegen: an `Ingest` carrying a
//! populated `SourceRef` union and a three-flag `Traits` value round-trips
//! through the FlatBuffer binary codec and through JSON.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use flatbed::{route, FlatbedRouteError, Request, Response};
use generated::test::{FileRef, Ingest, MailRef, SourceRef, Traits};
use serde_json::json;

#[route("/ingest", method = "POST")]
async fn echo(req: Request<Ingest>) -> Result<Response<Ingest>, FlatbedRouteError> {
    Ok(Response::ok(req.body))
}

async fn call_echo(body: Vec<u8>, content_type: &str) -> flatbed::ResponseParts {
    let parts = flatbed::RequestParts::new(flatbed::Method::POST, "/ingest".to_string());
    let ctx: std::sync::Arc<dyn std::any::Any + Send + Sync> = std::sync::Arc::new(());
    __hyper_handler_echo(parts, body, content_type, ctx)
        .await
        .expect("echo route succeeds")
}

fn mail_ingest() -> Ingest {
    Ingest {
        title: Some("Invoice".to_string()),
        ref_: Some(SourceRef::MailRef(MailRef {
            account: Some("acc".to_string()),
            message_id: Some("18f".to_string()),
        })),
        traits: Traits::Money | Traits::NeedsDecision | Traits::Gmail,
        default_traits: Traits::Client,
        history: Some(vec![Traits::empty(), Traits::Client | Traits::Money]),
    }
}

#[test]
fn union_and_combined_flags_round_trip_through_flatbuffers() {
    let original = mail_ingest();
    let bytes = original.to_flatbuffer();

    assert_eq!(Ingest::from_flatbuffer(&bytes).unwrap(), original);

    // The bytes are flatc's own union encoding: its accessors read them back.
    let fb = flatbed::flatbuffers::root::<generated::__fb::test::Ingest>(&bytes).unwrap();
    assert_eq!(fb.ref_type(), generated::__fb::test::SourceRef::MailRef);
    assert_eq!(fb.ref__as_mail_ref().unwrap().message_id(), Some("18f"));
    assert_eq!(fb.traits().bits(), 0b1110);
}

#[test]
fn each_union_variant_and_absence_round_trip_through_flatbuffers() {
    let file = Ingest {
        ref_: Some(SourceRef::FileRef(FileRef {
            path: Some("/tmp/a".to_string()),
        })),
        ..Ingest::default()
    };
    assert_eq!(
        Ingest::from_flatbuffer(&file.to_flatbuffer()).unwrap(),
        file
    );

    let absent = Ingest::default();
    let decoded = Ingest::from_flatbuffer(&absent.to_flatbuffer()).unwrap();
    assert_eq!(decoded.ref_, None);
    assert_eq!(decoded.default_traits, Traits::Money);
}

#[test]
fn flags_the_schema_does_not_name_survive_the_binary_codec() {
    let original = Ingest {
        traits: Traits::from_bits_retain(Traits::Client.bits() | 1 << 40),
        ..Ingest::default()
    };
    let decoded = Ingest::from_flatbuffer(&original.to_flatbuffer()).unwrap();
    assert_eq!(decoded.traits.bits(), 1 | 1 << 40);
}

#[test]
fn union_and_combined_flags_round_trip_through_json() {
    let original = mail_ingest();
    let value = serde_json::to_value(&original).unwrap();

    assert_eq!(
        value,
        json!({
            "title": "Invoice",
            "ref_type": "MailRef",
            "ref": { "account": "acc", "message_id": "18f" },
            "traits": ["Money", "NeedsDecision", "Gmail"],
            "default_traits": ["Client"],
            "history": [[], ["Client", "Money"]],
        })
    );
    assert_eq!(serde_json::from_value::<Ingest>(value).unwrap(), original);
}

#[test]
fn json_union_keys_are_order_independent_and_absent_when_none() {
    let value = json!({ "ref": { "path": "/tmp/a" }, "ref_type": "FileRef" });
    let decoded: Ingest = serde_json::from_value(value).unwrap();
    assert_eq!(
        decoded.ref_,
        Some(SourceRef::FileRef(FileRef {
            path: Some("/tmp/a".to_string())
        }))
    );

    let absent = serde_json::to_value(Ingest::default()).unwrap();
    assert!(absent.get("ref_type").is_none());
    assert!(absent.get("ref").is_none());
    assert_eq!(
        serde_json::from_value::<Ingest>(absent).unwrap(),
        Ingest::default()
    );

    // flatc spells an absent union as the `NONE` variant.
    let none: Ingest = serde_json::from_value(json!({ "ref_type": "NONE" })).unwrap();
    assert_eq!(none.ref_, None);
}

#[test]
fn json_missing_flags_take_the_field_default() {
    let decoded: Ingest = serde_json::from_value(json!({})).unwrap();
    assert_eq!(decoded.traits, Traits::empty());
    assert_eq!(decoded.default_traits, Traits::Money);
}

#[test]
fn json_rejects_malformed_unions() {
    let cases = [
        (
            json!({ "ref_type": "Bogus", "ref": {} }),
            "unknown variant `Bogus`",
        ),
        (json!({ "ref_type": "MailRef" }), "missing field `ref`"),
        (
            json!({ "ref": { "path": "x" } }),
            "missing field `ref_type`",
        ),
        (
            json!({ "ref_type": "MailRef", "ref": { "account": 7 } }),
            "invalid type",
        ),
    ];
    for (value, expected) in cases {
        let err = serde_json::from_value::<Ingest>(value.clone()).unwrap_err();
        assert!(
            err.to_string().contains(expected),
            "{value}: expected {expected:?}, got {err}"
        );
    }
}

#[test]
fn json_rejects_unknown_flag_names_and_bits() {
    let err = serde_json::from_value::<Ingest>(json!({ "traits": ["Money", "Urgent"] }))
        .unwrap_err()
        .to_string();
    assert!(err.contains("unknown variant `Urgent`"), "{err}");

    let unknown_bits = Ingest {
        traits: Traits::from_bits_retain(Traits::Client.bits() | 1 << 40),
        ..Ingest::default()
    };
    let err = serde_json::to_value(&unknown_bits).unwrap_err().to_string();
    assert!(err.contains("unknown Traits bits: 0x10000000000"), "{err}");
}

#[test]
fn json_flag_names_may_repeat() {
    let decoded: Ingest =
        serde_json::from_value(json!({ "traits": ["Gmail", "Client", "Gmail"] })).unwrap();
    assert_eq!(decoded.traits, Traits::Client | Traits::Gmail);
}

#[tokio::test]
async fn a_route_echoes_union_and_flags_in_both_codecs() {
    let original = mail_ingest();

    let json_response = call_echo(serde_json::to_vec(&original).unwrap(), "application/json").await;
    assert_eq!(json_response.content_type, "application/json");
    let echoed: Ingest = serde_json::from_slice(&json_response.body).unwrap();
    assert_eq!(echoed, original);

    let fb_response = call_echo(original.to_flatbuffer(), "application/x-flatbuffers").await;
    assert_eq!(
        Ingest::from_flatbuffer(&fb_response.body).unwrap(),
        original
    );
}
