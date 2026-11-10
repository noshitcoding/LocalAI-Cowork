use super::*;
use std::sync::Mutex;
const OP: &str = "35c78a7d-6bc1-4b01-8a52-ff7b831f29b1";
struct Fake {
    status: NativeSecretsStatus,
    previous: bool,
    error: Option<&'static str>,
    receipt: NativeSecretsReceipt,
    calls: Mutex<Vec<String>>,
}
impl Fake {
    fn new() -> Self {
        let status = NativeSecretsStatus {
            revision: 0,
            configured: false,
            key_available: false,
            pending_operation_id: None,
        };
        Self {
            status,
            previous: false,
            error: None,
            receipt: NativeSecretsReceipt {
                operation_id: OP.into(),
                operation_state: NativeSecretsSetupState::Activated,
                store: NativeSecretsStatus {
                    revision: 2,
                    configured: true,
                    key_available: true,
                    pending_operation_id: None,
                },
            },
            calls: Mutex::new(vec![]),
        }
    }
    fn record(&self, value: &str) {
        self.calls.lock().unwrap().push(value.into());
    }
    fn names(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}
impl Transport for Fake {
    async fn status(&self) -> Result<NativeSecretsStatus, ApiError> {
        self.record("status");
        Ok(self.status.clone())
    }
    async fn lookup(&self, key: &str) -> Result<NativeSecretsRequestLookup, ApiError> {
        assert_eq!(key, "request");
        self.record("lookup");
        Ok(NativeSecretsRequestLookup {
            receipt: self.previous.then(|| self.receipt.clone()),
        })
    }
    async fn setup(&self, input: &NativeSecretsSetup) -> Result<NativeSecretsReceipt, ApiError> {
        assert_eq!(input.idempotency_key, "request");
        assert_eq!(input.base_revision, 0);
        self.record("post-setup");
        if let Some(code) = self.error {
            Err(failure(code))
        } else {
            Ok(self.receipt.clone())
        }
    }
    async fn resolve(
        &self,
        action: Action,
        input: &NativeSecretsResolution,
    ) -> Result<NativeSecretsReceipt, ApiError> {
        assert_eq!(input.operation_id, OP);
        assert_eq!(input.base_revision, 1);
        self.record(if action == Action::Reconcile {
            "post-reconcile"
        } else {
            "post-abandon"
        });
        Ok(self.receipt.clone())
    }
}
fn input() -> NativeSecretsSetup {
    NativeSecretsSetup {
        idempotency_key: "request".into(),
        base_revision: 0,
    }
}
#[tokio::test]
async fn cancel_native_confirmation_never_sends_setup() {
    let client = Fake::new();
    assert!(
        setup(&client, &input(), |_| async {
            client.record("confirm");
            Ok(false)
        })
        .await
        .unwrap()
        .is_none()
    );
    assert_eq!(client.names(), ["lookup", "status", "confirm"]);
}
#[tokio::test]
async fn new_setup_requires_confirmation_and_never_retries_unknown_commit() {
    let mut client = Fake::new();
    client.error = Some("storage_unavailable");
    let result = setup(&client, &input(), |s| {
        assert!(!s.configured);
        client.record("confirm");
        async { Ok(true) }
    })
    .await;
    assert_eq!(result.unwrap_err().code, "storage_unavailable");
    assert_eq!(
        client.names(),
        ["lookup", "status", "confirm", "post-setup"]
    );
}
#[tokio::test]
async fn existing_request_still_validates_original_arguments_at_daemon_without_new_dialog() {
    let mut client = Fake::new();
    client.previous = true;
    client.error = Some("revision_conflict");
    let result = setup(&client, &input(), |_| async {
        panic!("must not reconfirm a recorded request")
    })
    .await;
    assert_eq!(result.unwrap_err().code, "revision_conflict");
    assert_eq!(client.names(), ["lookup", "post-setup"]);
}
#[tokio::test]
async fn stale_setup_is_rejected_before_dialog_or_mutation() {
    let mut client = Fake::new();
    client.status.revision = 2;
    assert_eq!(
        setup(&client, &input(), |_| async { panic!("stale") })
            .await
            .unwrap_err()
            .code,
        "revision_conflict"
    );
    assert_eq!(client.names(), ["lookup", "status"]);
}
#[tokio::test]
async fn pending_resolution_requires_exact_operation_and_native_confirmation() {
    let mut client = Fake::new();
    client.status.revision = 1;
    client.status.pending_operation_id = Some(OP.into());
    let mut input = NativeSecretsResolution {
        operation_id: uuid::Uuid::new_v4().to_string(),
        base_revision: 1,
    };
    assert_eq!(
        resolve(&client, Action::Reconcile, &input, |_| async {
            panic!("foreign")
        })
        .await
        .unwrap_err()
        .code,
        "revision_conflict"
    );
    input.operation_id = OP.into();
    assert!(
        resolve(&client, Action::Reconcile, &input, |_| async { Ok(false) })
            .await
            .unwrap()
            .is_none()
    );
    let receipt = resolve(&client, Action::Reconcile, &input, |_| async { Ok(true) })
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.operation_id, OP);
    assert_eq!(
        client.names(),
        ["status", "status", "status", "post-reconcile"]
    );
}
#[tokio::test]
async fn mismatched_resolution_receipt_is_uncertain_not_success() {
    let mut client = Fake::new();
    client.status.revision = 1;
    client.status.pending_operation_id = Some(OP.into());
    let input = NativeSecretsResolution {
        operation_id: OP.into(),
        base_revision: 1,
    };
    assert_eq!(
        resolve(&client, Action::Abandon, &input, |_| async { Ok(true) })
            .await
            .unwrap_err()
            .code,
        "secrets_setup_uncertain"
    );
    client.receipt.operation_state = NativeSecretsSetupState::Abandoned;
    assert!(
        resolve(&client, Action::Abandon, &input, |_| async { Ok(true) })
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        client.names(),
        ["status", "post-abandon", "status", "post-abandon"]
    );
}
