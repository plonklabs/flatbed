//! Encodes and decodes canonical fixtures for cross-language FlatBuffer codec
//! verification.
//!
//! `encode <Type>` prints the hex of a fixed sample value; `decode <Type> <hex>`
//! decodes the hex and exits non-zero unless it equals that same sample. A
//! companion Node script does the mirror image with the generated TS codec, so
//! together they prove the two implementations agree byte-for-byte.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use generated::test::{
    Address, AddressBook, Defaulted, LogEvent, Severity, TestResponse, UserRequest,
};

fn address() -> Address {
    Address {
        street: Some("1 Analytical Way".to_string()),
        city: Some("London".to_string()),
        zip_code: 12345,
    }
}

fn encode(ty: &str) -> Vec<u8> {
    match ty {
        "TestResponse" => TestResponse {
            message: Some("pong".to_string()),
            value: 9_000_000_000_000_000_000, // past 2^53 — exercises JS BigInt
            success: true,
        }
        .to_flatbuffer(),
        "UserRequest" => UserRequest {
            name: Some("Ada".to_string()),
            age: 36,
            address: Some(address()),
        }
        .to_flatbuffer(),
        "AddressBook" => AddressBook {
            owner: Some("Ada".to_string()),
            addresses: Some(vec![address(), address()]),
            contact_names: Some(vec!["Bob".to_string(), "Carol".to_string()]),
        }
        .to_flatbuffer(),
        "LogEvent" => LogEvent {
            message: Some("disk full".to_string()),
            severity: Severity::Error,
            history: Some(vec![Severity::Info, Severity::Warning, Severity::Error]),
        }
        .to_flatbuffer(),
        // Every value equals its schema default, so all fields are omitted on
        // the wire and the decoder must restore them.
        "Defaulted" => Defaulted {
            count: 25,
            flag: true,
            ratio: 1.5,
            level: Severity::Warning,
        }
        .to_flatbuffer(),
        other => panic!("unknown type {other}"),
    }
}

/// Decode `bytes` and assert it equals the same sample `encode` produces.
fn decode_and_check(ty: &str, bytes: &[u8]) {
    match ty {
        "TestResponse" => assert_eq!(
            TestResponse::from_flatbuffer(bytes).unwrap(),
            TestResponse::from_flatbuffer(&encode(ty)).unwrap()
        ),
        "UserRequest" => assert_eq!(
            UserRequest::from_flatbuffer(bytes).unwrap(),
            UserRequest::from_flatbuffer(&encode(ty)).unwrap()
        ),
        "AddressBook" => assert_eq!(
            AddressBook::from_flatbuffer(bytes).unwrap(),
            AddressBook::from_flatbuffer(&encode(ty)).unwrap()
        ),
        "LogEvent" => assert_eq!(
            LogEvent::from_flatbuffer(bytes).unwrap(),
            LogEvent::from_flatbuffer(&encode(ty)).unwrap()
        ),
        "Defaulted" => assert_eq!(
            Defaulted::from_flatbuffer(bytes).unwrap(),
            Defaulted::from_flatbuffer(&encode(ty)).unwrap()
        ),
        other => panic!("unknown type {other}"),
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn from_hex(s: &str) -> Vec<u8> {
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
        .collect()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("encode") => println!("{}", to_hex(&encode(&args[2]))),
        Some("decode") => {
            decode_and_check(&args[2], &from_hex(&args[3]));
            println!("ok");
        }
        _ => {
            eprintln!("usage: fb_roundtrip encode <Type> | decode <Type> <hex>");
            std::process::exit(2);
        }
    }
}
