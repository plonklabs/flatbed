//! Broker-free coverage of what `#[nats_route]` registers: the responder
//! entry, the worker that runs it, and the subject-conflict check that
//! `Flatbed::run` performs at startup.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use std::sync::Arc;

use flatbed::nats::HasNatsClient;
use flatbed::{nats_route, FlatbedRouteError, NatsRouteInfo, Request, Response};
use generated::test::{TestRequest, TestResponse};

struct Ctx(async_nats::Client);

impl HasNatsClient for Ctx {
    fn nats_client(&self) -> &async_nats::Client {
        &self.0
    }
}

#[nats_route("flatbed.registry.plain")]
async fn plain(
    _: Request<TestRequest, Arc<Ctx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Ok(Response::ok(TestResponse::default()))
}

#[nats_route("flatbed.registry.{tenant}.job.{job_id}", queue = "registry")]
async fn parameterized(
    _: Request<TestRequest, Arc<Ctx>>,
) -> Result<Response<TestResponse>, FlatbedRouteError> {
    Ok(Response::ok(TestResponse::default()))
}

fn route(subject: &str) -> &'static NatsRouteInfo {
    flatbed::get_nats_routes()
        .into_iter()
        .find(|route| route.subject == subject)
        .unwrap_or_else(|| panic!("#[nats_route] must register '{subject}'"))
}

#[test]
fn a_literal_subject_registers_with_no_queue_group_and_no_params() {
    let route = route("flatbed.registry.plain");

    assert_eq!(route.wire_subject, "flatbed.registry.plain");
    assert_eq!(route.queue, None);
    assert!(route.params.is_empty());
    assert_eq!(route.request_type, "TestRequest");
    assert_eq!(route.response_type, "TestResponse");
}

#[test]
fn token_segments_register_as_wildcards_with_their_capture_indexes() {
    let route = route("flatbed.registry.{tenant}.job.{job_id}");

    assert_eq!(route.wire_subject, "flatbed.registry.*.job.*");
    assert_eq!(route.queue, Some("registry"));
    assert_eq!(route.params, [("tenant", 2), ("job_id", 4)]);
}

/// Each responder also registers as a worker, which is what makes
/// `Flatbed::run` spawn it — registration without this would compile and
/// then answer nothing.
#[test]
fn each_responder_registers_a_worker_that_runs_it() {
    let names: Vec<&str> = flatbed::get_workers().iter().map(|w| w.name).collect();

    assert!(names.contains(&"nats_route:flatbed.registry.plain"));
    assert!(names.contains(&"nats_route:flatbed.registry.{tenant}.job.{job_id}"));
}

#[test]
fn distinct_subjects_pass_the_startup_conflict_check() {
    flatbed::validate_nats_routes().expect("distinct subjects must not conflict");
}
