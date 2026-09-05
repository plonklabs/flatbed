//! The generated codec traits, exercised through the traits rather than
//! through the inherent methods they delegate to.
//!
//! Generic code reaches a body type's codec only through `ToFlatBuffer` /
//! `FromFlatBuffer`, and each generated impl shares its name with an inherent
//! method of the same signature. Calling through the trait is what
//! distinguishes a delegation that lands on the inherent method from one that
//! lands back on itself.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use flatbed::{FromFlatBuffer, ToFlatBuffer};
use generated::test::TestRequest;

fn round_trip<T: ToFlatBuffer + FromFlatBuffer>(value: &T) -> T {
    T::from_flatbuffer(&ToFlatBuffer::to_flatbuffer(value)).expect("flatbuffer decode failed")
}

#[test]
fn a_generated_type_round_trips_through_the_codec_traits() {
    let original = TestRequest {
        message: Some("hello".to_string()),
        value: 42,
    };

    assert_eq!(round_trip(&original), original);
}

/// A body-less response is as decodable as it is encodable, so a subject
/// answering with no body has a type its caller can bind.
#[test]
fn the_unit_body_round_trips_through_the_codec_traits() {
    assert_eq!(round_trip(&()), ());
    assert!(ToFlatBuffer::to_flatbuffer(&()).is_empty());
}
