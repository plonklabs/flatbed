//! The Kubernetes and NATS `register_*!` macros have no other coverage — the
//! reconciler tests need a live cluster and the stream tests need a broker —
//! so this pins that both arms of each macro expand, type-check, and submit a
//! worker (carrying the restart policy) alongside a drain.

use std::sync::Arc;
use std::time::Duration;

use flatbed::k8s::{
    HasKubeClient, HasLeaderElection, KubeNativeReconciler, KubeWatcher, ReconcileError,
};
use flatbed::kv::KvWorker;
use flatbed::nats::{HasJetStream, StreamWorker};
use flatbed::FlatbedWorkerError;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::runtime::controller::Action;
use tokio::sync::watch;

struct Ctx {
    kube_client: kube::Client,
    leader_rx: watch::Receiver<bool>,
}

impl HasJetStream for Ctx {
    fn jetstream(&self) -> &async_nats::jetstream::Context {
        unimplemented!("the registered workers are never run here")
    }
}

impl HasKubeClient for Ctx {
    fn kube_client(&self) -> &kube::Client {
        &self.kube_client
    }
}

impl HasLeaderElection for Ctx {
    fn is_leader_rx(&self) -> watch::Receiver<bool> {
        self.leader_rx.clone()
    }
}

#[derive(Default)]
struct Rec;

impl flatbed::KubeReconciler for Rec {
    type Resource = ConfigMap;
    type Context = Ctx;
    type Error = ReconcileError;
    const NAME: &'static str = "macros-reconciler";
    const STREAM: &'static str = "MACROS";
    const STREAM_SUBJECTS: &'static str = "macros.>";
    fn reconcile(
        &self,
        _ctx: Arc<Ctx>,
        _obj: Arc<ConfigMap>,
    ) -> flatbed::BoxFuture<Result<Action, Self::Error>> {
        Box::pin(async { Ok(Action::requeue(Duration::from_secs(1))) })
    }
}

flatbed::register_kube_reconciler!(Rec, Ctx);

#[derive(Default)]
struct NativeRec;

impl KubeNativeReconciler for NativeRec {
    type Resource = ConfigMap;
    type Context = Ctx;
    type Error = ReconcileError;
    const NAME: &'static str = "macros-native";
    const RESTART: Option<flatbed::RestartPolicy> = Some(flatbed::RestartPolicy::backoff(
        Duration::from_secs(1),
        Duration::from_secs(2),
    ));
    fn reconcile(
        &self,
        _ctx: Arc<Ctx>,
        _obj: Arc<ConfigMap>,
    ) -> flatbed::BoxFuture<Result<Action, Self::Error>> {
        Box::pin(async { Ok(Action::requeue(Duration::from_secs(1))) })
    }
}

flatbed::register_kube_native_reconciler!(NativeRec, Ctx);

#[derive(Default)]
struct Watch;

impl KubeWatcher for Watch {
    type Resource = ConfigMap;
    type Context = Ctx;
    const NAME: &'static str = "macros-watcher";
    fn on_apply(&self, _ctx: Arc<Ctx>, _obj: ConfigMap) -> flatbed::BoxFuture<()> {
        Box::pin(async {})
    }
    fn on_delete(&self, _ctx: Arc<Ctx>, _obj: ConfigMap) -> flatbed::BoxFuture<()> {
        Box::pin(async {})
    }
    fn on_init_apply(&self, _ctx: Arc<Ctx>, _obj: ConfigMap) -> flatbed::BoxFuture<()> {
        Box::pin(async {})
    }
}

flatbed::register_kube_watcher!(Watch, Ctx);

#[derive(Default)]
struct Stream;

impl StreamWorker for Stream {
    type Message = Vec<u8>;
    type Context = Ctx;
    type ParseError = String;
    const NAME: &'static str = "macros-stream";
    const STREAM: &'static str = "MACROS";
    const SUBJECT: &'static str = "macros.stream";
    const RESTART: Option<flatbed::RestartPolicy> = Some(flatbed::RestartPolicy::backoff(
        Duration::from_secs(1),
        Duration::from_secs(2),
    ));
    fn handle(
        &self,
        _ctx: Arc<Ctx>,
        _msg: Vec<u8>,
    ) -> flatbed::BoxFuture<flatbed::nats::NatsResult> {
        Box::pin(async { flatbed::nats::NatsResult::Ack })
    }
    fn parse_message(bytes: &[u8]) -> Result<Vec<u8>, String> {
        Ok(bytes.to_vec())
    }
}

flatbed::register_stream_worker!(Stream, Ctx);

#[derive(Default)]
struct Kv;

impl KvWorker for Kv {
    type Value = Vec<u8>;
    type Context = Ctx;
    type ParseError = String;
    const NAME: &'static str = "macros-kv";
    const BUCKET: &'static str = "macros";
    fn on_put(&self, _ctx: Arc<Ctx>, _key: String, _value: Vec<u8>) -> flatbed::BoxFuture<()> {
        Box::pin(async {})
    }
    fn on_delete(&self, _ctx: Arc<Ctx>, _key: String) -> flatbed::BoxFuture<()> {
        Box::pin(async {})
    }
    fn parse_value(bytes: &[u8]) -> Result<Vec<u8>, String> {
        Ok(bytes.to_vec())
    }
    fn drain(&self, _ctx: Arc<Ctx>) -> flatbed::BoxFuture<Result<(), FlatbedWorkerError>> {
        Box::pin(async { Ok(()) })
    }
}

flatbed::register_kv_worker!(Kv, Ctx);

#[test]
fn macros_register_workers_and_drains() {
    let with_policy = ["macros-native", "macros-stream"];
    let without_policy = ["macros-reconciler", "macros-watcher", "macros-kv"];

    for name in with_policy.iter().chain(&without_policy) {
        let worker = flatbed::get_workers()
            .into_iter()
            .find(|w| w.name == *name)
            .unwrap_or_else(|| panic!("{name} did not register a worker"));
        assert_eq!(
            worker.restart.is_some(),
            with_policy.contains(name),
            "{name} restart policy did not survive macro expansion"
        );
        assert!(
            flatbed::get_worker_drains().iter().any(|d| d.name == *name),
            "{name} did not register a drain"
        );
    }
}
