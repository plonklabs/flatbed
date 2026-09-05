//! The Kubernetes `register_*!` macros have no other coverage — the
//! reconciler tests need a live cluster — so this pins that each one expands,
//! type-checks, and submits both a worker and a drain.

use std::sync::Arc;
use std::time::Duration;

use flatbed::k8s::{
    HasKubeClient, HasLeaderElection, KubeNativeReconciler, KubeWatcher, ReconcileError,
};
use flatbed::nats::HasJetStream;
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
    fn reconcile(
        &self,
        _ctx: Arc<Ctx>,
        _obj: Arc<ConfigMap>,
    ) -> flatbed::BoxFuture<Result<Action, Self::Error>> {
        Box::pin(async { Ok(Action::requeue(Duration::from_secs(1))) })
    }
}

flatbed::register_kube_native_reconciler!(
    NativeRec,
    Ctx,
    restart = flatbed::RestartPolicy::backoff(Duration::from_secs(1), Duration::from_secs(2))
);

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

#[test]
fn macros_register_workers_and_drains() {
    for name in ["macros-reconciler", "macros-native", "macros-watcher"] {
        assert!(flatbed::get_workers().iter().any(|w| w.name == name));
        assert!(flatbed::get_worker_drains().iter().any(|d| d.name == name));
    }
}
