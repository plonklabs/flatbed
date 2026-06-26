//! Integration tests for the trait-based KubeReconciler system.
//!
//! These tests exercise the `reconcile_with_finalizer` orchestration logic
//! against a real Kubernetes cluster (k3d). They are marked `#[ignore]`
//! and require a running cluster to pass.
//!
//! Run with:
//!     cargo test -p flatbed --features nats,k8s --test reconciler_tests -- --ignored

use std::sync::{Arc, Mutex};
use std::time::Duration;

use flatbed::k8s::{
    reconcile_with_finalizer, HasKubeClient, HasLeaderElection, KubeReconciler, ReconcileError,
};
use flatbed::nats::HasJetStream;
use k8s_openapi::api::core::v1::ConfigMap;
use kube::api::{Api, DeleteParams, ObjectMeta, PostParams};
use kube::runtime::controller::Action;
use kube::Resource;
use tokio::sync::watch;

// ============================================================================
// Test infrastructure
// ============================================================================

/// Shared call log used by tracking reconcilers.
type CallLog = Arc<Mutex<Vec<String>>>;

/// Test context that provides a real kube::Client and stubs for JetStream/leader election.
///
/// JetStream is not actually used in reconcile_with_finalizer, but the trait
/// bounds require it. The stub panics if accessed — the tests only exercise
/// code paths that do not touch JetStream.
struct TestCtx {
    kube_client: kube::Client,
    leader_rx: watch::Receiver<bool>,
}

impl HasJetStream for TestCtx {
    fn jetstream(&self) -> &async_nats::jetstream::Context {
        panic!("StubJetStream: reconcile_with_finalizer should not access JetStream");
    }
}

impl HasKubeClient for TestCtx {
    fn kube_client(&self) -> &kube::Client {
        &self.kube_client
    }
}

impl HasLeaderElection for TestCtx {
    fn is_leader_rx(&self) -> watch::Receiver<bool> {
        self.leader_rx.clone()
    }
}

/// Build a TestCtx with a real kube client.
async fn test_ctx() -> Arc<TestCtx> {
    let client = kube::Client::try_default()
        .await
        .expect("Failed to create kube client -- is KUBECONFIG set?");
    let (_tx, rx) = watch::channel(true);
    Arc::new(TestCtx {
        kube_client: client,
        leader_rx: rx,
    })
}

/// Create a ConfigMap in the default namespace for testing.
async fn create_test_configmap(client: &kube::Client, name: &str) -> ConfigMap {
    let api: Api<ConfigMap> = Api::namespaced(client.clone(), "default");
    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            ..Default::default()
        },
        ..Default::default()
    };
    api.create(&PostParams::default(), &cm)
        .await
        .unwrap_or_else(|e| panic!("Failed to create ConfigMap {name}: {e}"))
}

/// Delete a ConfigMap in the default namespace, ignoring errors.
async fn delete_test_configmap(client: &kube::Client, name: &str) {
    let api: Api<ConfigMap> = Api::namespaced(client.clone(), "default");
    let _ = api.delete(name, &DeleteParams::default()).await;
}

/// Re-read a ConfigMap from the API server.
async fn get_configmap(client: &kube::Client, name: &str) -> ConfigMap {
    let api: Api<ConfigMap> = Api::namespaced(client.clone(), "default");
    api.get(name)
        .await
        .unwrap_or_else(|e| panic!("Failed to get ConfigMap {name}: {e}"))
}

// ============================================================================
// Reconciler implementations for testing
// ============================================================================

/// Reconciler with FINALIZER = None.
struct NoFinalizerReconciler {
    call_log: CallLog,
}

impl KubeReconciler for NoFinalizerReconciler {
    type Resource = ConfigMap;
    type Context = TestCtx;
    type Error = ReconcileError;

    const NAME: &'static str = "no-finalizer-reconciler";
    const STREAM: &'static str = "TEST";
    const STREAM_SUBJECTS: &'static str = "test.>";

    fn reconcile(
        &self,
        _ctx: Arc<Self::Context>,
        _obj: Arc<Self::Resource>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Action, Self::Error>> + Send>>
    {
        let log = Arc::clone(&self.call_log);
        Box::pin(async move {
            log.lock().unwrap().push("reconcile".to_string());
            Ok(Action::requeue(Duration::from_secs(300)))
        })
    }

    fn cleanup(
        &self,
        _ctx: Arc<Self::Context>,
        _obj: Arc<Self::Resource>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Self::Error>> + Send>> {
        let log = Arc::clone(&self.call_log);
        Box::pin(async move {
            log.lock().unwrap().push("cleanup".to_string());
            Ok(())
        })
    }
}

/// Reconciler with a finalizer configured.
struct WithFinalizerReconciler {
    call_log: CallLog,
}

impl KubeReconciler for WithFinalizerReconciler {
    type Resource = ConfigMap;
    type Context = TestCtx;
    type Error = ReconcileError;

    const NAME: &'static str = "with-finalizer-reconciler";
    const STREAM: &'static str = "TEST";
    const STREAM_SUBJECTS: &'static str = "test.>";
    const FINALIZER: Option<&'static str> = Some("flatbed.test/cleanup");

    fn reconcile(
        &self,
        _ctx: Arc<Self::Context>,
        _obj: Arc<Self::Resource>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Action, Self::Error>> + Send>>
    {
        let log = Arc::clone(&self.call_log);
        Box::pin(async move {
            log.lock().unwrap().push("reconcile".to_string());
            Ok(Action::requeue(Duration::from_secs(300)))
        })
    }

    fn cleanup(
        &self,
        _ctx: Arc<Self::Context>,
        _obj: Arc<Self::Resource>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), Self::Error>> + Send>> {
        let log = Arc::clone(&self.call_log);
        Box::pin(async move {
            log.lock().unwrap().push("cleanup".to_string());
            Ok(())
        })
    }
}

// ============================================================================
// Tests
// ============================================================================

/// With FINALIZER = None, reconcile() should be called directly.
/// No kube API calls for finalizer management should occur.
#[tokio::test]
#[ignore]
async fn no_finalizer_calls_reconcile_directly() {
    let ctx = test_ctx().await;
    let name = "flatbed-test-no-fin";

    // Ensure clean state
    delete_test_configmap(ctx.kube_client(), name).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let cm = create_test_configmap(ctx.kube_client(), name).await;

    let call_log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let reconciler = NoFinalizerReconciler {
        call_log: Arc::clone(&call_log),
    };

    let result = reconcile_with_finalizer(&reconciler, Arc::clone(&ctx), Arc::new(cm)).await;
    assert!(result.is_ok(), "reconcile should succeed");

    {
        let log = call_log.lock().unwrap();
        assert_eq!(*log, vec!["reconcile"], "Only reconcile should be called");
    }

    // Verify no finalizer was added
    let refreshed = get_configmap(ctx.kube_client(), name).await;
    let finalizers = refreshed.meta().finalizers.as_ref();
    assert!(
        finalizers.is_none() || finalizers.unwrap().is_empty(),
        "No finalizer should be present on the resource"
    );

    // Cleanup
    delete_test_configmap(ctx.kube_client(), name).await;
}

/// With FINALIZER = Some(...) and no deletion timestamp, the finalizer should
/// be added and reconcile() should be called.
#[tokio::test]
#[ignore]
async fn with_finalizer_adds_finalizer_and_reconciles() {
    let ctx = test_ctx().await;
    let name = "flatbed-test-with-fin";

    // Ensure clean state
    delete_test_configmap(ctx.kube_client(), name).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let cm = create_test_configmap(ctx.kube_client(), name).await;

    let call_log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let reconciler = WithFinalizerReconciler {
        call_log: Arc::clone(&call_log),
    };

    let result = reconcile_with_finalizer(&reconciler, Arc::clone(&ctx), Arc::new(cm)).await;
    assert!(result.is_ok(), "reconcile should succeed");

    {
        let log = call_log.lock().unwrap();
        assert_eq!(
            *log,
            vec!["reconcile"],
            "reconcile should be called (not cleanup)"
        );
    }

    // Verify the finalizer was added to the resource
    let refreshed = get_configmap(ctx.kube_client(), name).await;
    assert!(
        flatbed::k8s::has_finalizer(&refreshed, "flatbed.test/cleanup"),
        "Finalizer should be present after reconcile"
    );

    // Cleanup: remove finalizer first so the resource can be deleted
    let api: Api<ConfigMap> = Api::namespaced(ctx.kube_client().clone(), "default");
    let patch = serde_json::json!({
        "metadata": {
            "finalizers": []
        }
    });
    let _ = api
        .patch(
            name,
            &kube::api::PatchParams::default(),
            &kube::api::Patch::Merge(&patch),
        )
        .await;
    delete_test_configmap(ctx.kube_client(), name).await;
}

/// With FINALIZER = Some(...), deletion timestamp set, and finalizer present,
/// cleanup() should be called and the finalizer should be removed.
#[tokio::test]
#[ignore]
async fn with_finalizer_and_deletion_calls_cleanup() {
    let ctx = test_ctx().await;
    let name = "flatbed-test-del-fin";

    // Ensure clean state
    delete_test_configmap(ctx.kube_client(), name).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let cm = create_test_configmap(ctx.kube_client(), name).await;

    // First, add the finalizer via reconcile
    let call_log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let reconciler = WithFinalizerReconciler {
        call_log: Arc::clone(&call_log),
    };
    reconcile_with_finalizer(&reconciler, Arc::clone(&ctx), Arc::new(cm))
        .await
        .unwrap();

    // Verify finalizer is present
    let refreshed = get_configmap(ctx.kube_client(), name).await;
    assert!(
        flatbed::k8s::has_finalizer(&refreshed, "flatbed.test/cleanup"),
        "Finalizer should be present before delete"
    );

    // Now request deletion -- the resource won't be deleted because it has a finalizer
    let api: Api<ConfigMap> = Api::namespaced(ctx.kube_client().clone(), "default");
    api.delete(name, &DeleteParams::default()).await.unwrap();

    // Give the API server a moment to set the deletion timestamp
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Re-read the resource -- it should have a deletion timestamp but still exist
    let deleting_cm = get_configmap(ctx.kube_client(), name).await;
    assert!(
        deleting_cm.meta().deletion_timestamp.is_some(),
        "Resource should have a deletion timestamp"
    );

    // Clear the call log and run reconcile_with_finalizer again
    call_log.lock().unwrap().clear();
    let result =
        reconcile_with_finalizer(&reconciler, Arc::clone(&ctx), Arc::new(deleting_cm)).await;
    assert!(result.is_ok(), "reconcile should succeed during cleanup");

    {
        let log = call_log.lock().unwrap();
        assert_eq!(
            *log,
            vec!["cleanup"],
            "cleanup should be called (not reconcile) when deleting"
        );
    }

    // After cleanup, the finalizer should be removed and the resource should be gone
    // (or about to be garbage collected by the API server)
    tokio::time::sleep(Duration::from_millis(500)).await;
    let api: Api<ConfigMap> = Api::namespaced(ctx.kube_client().clone(), "default");
    let get_result = api.get(name).await;
    assert!(
        get_result.is_err(),
        "Resource should be deleted after finalizer removal"
    );
}

/// With FINALIZER = Some(...), deletion timestamp set, but finalizer NOT present
/// on the resource, it should return await_change without calling cleanup.
#[tokio::test]
#[ignore]
async fn deletion_without_finalizer_skips_cleanup() {
    let ctx = test_ctx().await;
    let name = "flatbed-test-del-nofin";

    // Ensure clean state
    delete_test_configmap(ctx.kube_client(), name).await;
    tokio::time::sleep(Duration::from_millis(500)).await;

    // Create a ConfigMap with a different finalizer so it won't be deleted immediately
    let api: Api<ConfigMap> = Api::namespaced(ctx.kube_client().clone(), "default");
    let cm = ConfigMap {
        metadata: ObjectMeta {
            name: Some(name.to_string()),
            finalizers: Some(vec!["other.finalizer/hold".to_string()]),
            ..Default::default()
        },
        ..Default::default()
    };
    api.create(&PostParams::default(), &cm).await.unwrap();

    // Delete the resource -- it will have a deletion timestamp but not be removed
    // because of the "other.finalizer/hold"
    api.delete(name, &DeleteParams::default()).await.unwrap();
    tokio::time::sleep(Duration::from_millis(500)).await;

    let deleting_cm = get_configmap(ctx.kube_client(), name).await;
    assert!(
        deleting_cm.meta().deletion_timestamp.is_some(),
        "Resource should have a deletion timestamp"
    );
    assert!(
        !flatbed::k8s::has_finalizer(&deleting_cm, "flatbed.test/cleanup"),
        "Our finalizer should NOT be present"
    );

    let call_log: CallLog = Arc::new(Mutex::new(Vec::new()));
    let reconciler = WithFinalizerReconciler {
        call_log: Arc::clone(&call_log),
    };

    let result =
        reconcile_with_finalizer(&reconciler, Arc::clone(&ctx), Arc::new(deleting_cm)).await;
    assert!(result.is_ok(), "reconcile should succeed");

    {
        let log = call_log.lock().unwrap();
        assert!(
            log.is_empty(),
            "Neither reconcile nor cleanup should be called when resource is deleting but our finalizer is absent"
        );
    }

    // Cleanup: remove the holding finalizer so the resource can be deleted
    let patch = serde_json::json!({
        "metadata": {
            "finalizers": []
        }
    });
    let _ = api
        .patch(
            name,
            &kube::api::PatchParams::default(),
            &kube::api::Patch::Merge(&patch),
        )
        .await;
}
