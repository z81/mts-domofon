use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use axum::{
    Router,
    body::Body,
    http::{Request, StatusCode, header},
};
use chrono::{DateTime, TimeZone, Utc};
use domru_mts::{
    app::build_router_with_config,
    config::AppConfig,
    domain::{
        auth::{AuthFlavor, PersistedSession, TokenBundle},
        auto_open::{AutoOpenMode, AutoOpenResultStatus},
        call::{CallId, CallMedia},
    },
    error::{AppError, ErrorCode},
    service::{
        auto_open::{AutoOpenClock, AutoOpenService, run_auto_open_watcher},
        calls::{CallClock, CallOpenProvider, CallService, CallTransport, ObservedIncomingCall},
    },
    state::AppState,
    storage::SecretStore,
};
use futures_util::future::join_all;
use http_body_util::BodyExt;
use secrecy::SecretString;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

#[derive(Clone)]
struct AdjustableClock(Arc<Mutex<DateTime<Utc>>>);

impl AdjustableClock {
    fn new(now: DateTime<Utc>) -> Self {
        Self(Arc::new(Mutex::new(now)))
    }

    fn advance(&self, duration: chrono::TimeDelta) {
        let mut now = self.0.lock().unwrap();
        *now = now.checked_add_signed(duration).unwrap();
    }
}

impl AutoOpenClock for AdjustableClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

impl CallClock for AdjustableClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

fn session(authenticated_at: DateTime<Utc>) -> PersistedSession {
    PersistedSession {
        tokens: TokenBundle {
            access_token: SecretString::from("test-access"),
            refresh_token: SecretString::from("test-refresh"),
            web_sso_access_token: Some(SecretString::from("test-sso")),
            device_token: Some(SecretString::from("test-device")),
        },
        auth_flavor: AuthFlavor::WebSso,
        authenticated_at,
    }
}

async fn make_service(store: SecretStore, clock: AdjustableClock) -> Arc<AutoOpenService> {
    Arc::new(
        AutoOpenService::restore_for_test(
            store,
            Arc::new(clock),
            Duration::from_secs(10),
            Duration::from_secs(30 * 60),
            16,
            32,
        )
        .await
        .unwrap(),
    )
}

async fn arm(service: &AutoOpenService, mode: AutoOpenMode, dismiss_after_open: bool) {
    let intent = service
        .create_intent(mode, dismiss_after_open)
        .await
        .unwrap();
    service.execute_intent(&intent.intent_id).await.unwrap();
}

#[tokio::test]
async fn policy_is_default_off_and_next_call_is_confirmed_one_use_and_consumed_before_dispatch() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 7, 12, 0, 0).unwrap();
    let clock = AdjustableClock::new(now);
    let store = SecretStore::open(dir.path(), Some([41_u8; 32]))
        .await
        .unwrap();
    store.save_session(&session(now)).await.unwrap();
    let service = make_service(store, clock.clone()).await;

    assert_eq!(service.policy().await.unwrap().mode, AutoOpenMode::Disabled);
    let expiring = service
        .create_intent(AutoOpenMode::Until, false)
        .await
        .unwrap();
    assert_eq!(service.policy().await.unwrap().mode, AutoOpenMode::Disabled);
    clock.advance(chrono::TimeDelta::seconds(11));
    assert_eq!(
        service
            .execute_intent(&expiring.intent_id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::AutoOpenIntentNotFound
    );

    let intent = service
        .create_intent(AutoOpenMode::NextCall, true)
        .await
        .unwrap();
    let armed = service.execute_intent(&intent.intent_id).await.unwrap();
    assert_eq!(armed.mode, AutoOpenMode::NextCall);
    assert!(armed.dismiss_after_open);
    assert_eq!(
        service
            .execute_intent(&intent.intent_id)
            .await
            .unwrap_err()
            .code,
        ErrorCode::AutoOpenIntentNotFound
    );

    let call_id = CallId::new();
    let reservation = service
        .reserve_call(call_id.clone())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reservation.call_id(), &call_id);
    assert!(reservation.dismiss_after_open());
    assert_eq!(service.policy().await.unwrap().mode, AutoOpenMode::Disabled);
    assert!(service.reserve_call(call_id).await.unwrap().is_none());
    assert!(service.reserve_call(CallId::new()).await.unwrap().is_none());

    let raw = tokio::fs::read(dir.path().join("auto-open.enc"))
        .await
        .unwrap();
    assert!(raw.starts_with(b"VDM1"));
    let raw = String::from_utf8_lossy(&raw);
    assert!(!raw.contains("next_call"));
    assert!(!raw.contains("until"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn until_policy_deduplicates_concurrently_survives_restart_and_clears_on_expiry_or_session_change()
 {
    let dir = tempfile::tempdir().unwrap();
    let key = [42_u8; 32];
    let now = Utc.with_ymd_and_hms(2026, 8, 7, 13, 0, 0).unwrap();
    let clock = AdjustableClock::new(now);
    let store = SecretStore::open(dir.path(), Some(key)).await.unwrap();
    store.save_session(&session(now)).await.unwrap();
    let service = make_service(store.clone(), clock.clone()).await;
    arm(&service, AutoOpenMode::Until, false).await;
    let policy = service.policy().await.unwrap();
    assert_eq!(policy.mode, AutoOpenMode::Until);
    assert_eq!(policy.until, Some(now + chrono::TimeDelta::minutes(30)));

    let call_id = CallId::new();
    let attempts = join_all((0..12).map(|_| {
        let service = Arc::clone(&service);
        let call_id = call_id.clone();
        async move { service.reserve_call(call_id).await.unwrap() }
    }))
    .await;
    assert_eq!(attempts.iter().filter(|value| value.is_some()).count(), 1);

    let raw = tokio::fs::read(dir.path().join("auto-open.enc"))
        .await
        .unwrap();
    let raw_text = String::from_utf8_lossy(&raw);
    assert!(!raw_text.contains("until"));
    assert!(!raw_text.contains(&call_id.to_string()));

    let reopened_store = SecretStore::open(dir.path(), Some(key)).await.unwrap();
    let reopened = make_service(reopened_store.clone(), clock.clone()).await;
    assert_eq!(reopened.policy().await.unwrap().mode, AutoOpenMode::Until);
    assert!(reopened.reserve_call(call_id).await.unwrap().is_none());
    assert!(
        reopened
            .reserve_call(CallId::new())
            .await
            .unwrap()
            .is_some()
    );

    clock.advance(chrono::TimeDelta::minutes(31));
    assert_eq!(
        reopened.policy().await.unwrap().mode,
        AutoOpenMode::Disabled
    );
    assert!(!dir.path().join("auto-open.enc").exists());

    clock.advance(chrono::TimeDelta::minutes(1));
    arm(&reopened, AutoOpenMode::Until, false).await;
    reopened_store
        .save_session(&session(now + chrono::TimeDelta::hours(2)))
        .await
        .unwrap();
    assert_eq!(
        reopened.policy().await.unwrap().mode,
        AutoOpenMode::Disabled
    );
    assert!(!dir.path().join("auto-open.enc").exists());
}

#[tokio::test]
async fn completion_is_safe_and_failed_or_unknown_calls_are_never_reserved_again() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 7, 14, 0, 0).unwrap();
    let clock = AdjustableClock::new(now);
    let store = SecretStore::open(dir.path(), Some([43_u8; 32]))
        .await
        .unwrap();
    store.save_session(&session(now)).await.unwrap();
    let service = make_service(store, clock).await;
    arm(&service, AutoOpenMode::Until, true).await;

    let unknown_id = CallId::new();
    let unknown = service
        .reserve_call(unknown_id.clone())
        .await
        .unwrap()
        .unwrap();
    service
        .complete(
            unknown,
            Err(AppError::new(
                ErrorCode::MutationOutcomeUnknown,
                "Synthetic ambiguous result.",
                false,
            )),
        )
        .await
        .unwrap();
    assert_eq!(
        service.policy().await.unwrap().last_result.unwrap().status,
        AutoOpenResultStatus::OutcomeUnknown
    );
    assert!(service.reserve_call(unknown_id).await.unwrap().is_none());

    let failed_id = CallId::new();
    let failed = service
        .reserve_call(failed_id.clone())
        .await
        .unwrap()
        .unwrap();
    service
        .complete(
            failed,
            Err(AppError::new(
                ErrorCode::UpstreamUnavailable,
                "Synthetic failure.",
                true,
            )),
        )
        .await
        .unwrap();
    assert_eq!(
        service.policy().await.unwrap().last_result.unwrap().status,
        AutoOpenResultStatus::Failed
    );
    assert!(service.reserve_call(failed_id).await.unwrap().is_none());
}

#[tokio::test]
async fn completion_from_a_replaced_policy_cannot_overwrite_the_new_policy() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 7, 14, 30, 0).unwrap();
    let clock = AdjustableClock::new(now);
    let store = SecretStore::open(dir.path(), Some([46_u8; 32]))
        .await
        .unwrap();
    store.save_session(&session(now)).await.unwrap();
    let service = make_service(store, clock).await;

    arm(&service, AutoOpenMode::Until, false).await;
    let stale = service.reserve_call(CallId::new()).await.unwrap().unwrap();
    arm(&service, AutoOpenMode::NextCall, true).await;
    service.complete(stale, Ok(())).await.unwrap();

    let policy = service.policy().await.unwrap();
    assert_eq!(policy.mode, AutoOpenMode::NextCall);
    assert!(policy.last_result.is_none());
}

#[derive(Default)]
struct RecordingTransport {
    ignored: Mutex<Vec<CallId>>,
}

#[async_trait]
impl CallTransport for RecordingTransport {
    async fn answer(&self, _call_id: &CallId, _answer_sdp: &str) -> Result<(), AppError> {
        Ok(())
    }

    async fn decline(&self, _call_id: &CallId) -> Result<(), AppError> {
        Ok(())
    }

    async fn ignore(&self, call_id: &CallId) -> Result<(), AppError> {
        self.ignored.lock().unwrap().push(call_id.clone());
        Ok(())
    }

    async fn hangup(&self, _call_id: &CallId) -> Result<(), AppError> {
        Ok(())
    }
}

struct ScriptedOpener {
    results: Mutex<VecDeque<Result<(), AppError>>>,
    calls: Mutex<usize>,
}

#[async_trait]
impl CallOpenProvider for ScriptedOpener {
    async fn open(
        &self,
        _intercom_id: &SecretString,
        _call_session_id: &SecretString,
    ) -> Result<(), AppError> {
        *self.calls.lock().unwrap() += 1;
        self.results.lock().unwrap().pop_front().unwrap_or_else(|| {
            Err(AppError::new(
                ErrorCode::Internal,
                "Unexpected duplicate synthetic open.",
                false,
            ))
        })
    }
}

async fn wait_until(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !predicate() {
            tokio::task::yield_now().await;
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn watcher_dispatches_once_and_dismisses_only_after_definitive_success() {
    for (result, expected_ignored, expected_status) in [
        (Ok(()), 1, AutoOpenResultStatus::Opened),
        (
            Err(AppError::new(
                ErrorCode::UpstreamUnavailable,
                "Synthetic provider failure.",
                true,
            )),
            0,
            AutoOpenResultStatus::Failed,
        ),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 7, 15, 0, 0).unwrap();
        let clock = AdjustableClock::new(now);
        let store = SecretStore::open(dir.path(), Some([44_u8; 32]))
            .await
            .unwrap();
        store.save_session(&session(now)).await.unwrap();
        let auto_open = make_service(store, clock.clone()).await;
        arm(&auto_open, AutoOpenMode::Until, true).await;

        let transport = Arc::new(RecordingTransport::default());
        let opener = Arc::new(ScriptedOpener {
            results: Mutex::new(VecDeque::from([result])),
            calls: Mutex::new(0),
        });
        let calls = Arc::new(
            CallService::new_for_test(
                transport.clone(),
                opener.clone(),
                Arc::new(clock),
                Duration::from_secs(30),
                16,
            )
            .unwrap(),
        );
        let call_id = CallId::new();
        calls
            .observe_incoming(ObservedIncomingCall::new(
                call_id,
                CallMedia::AudioVideo,
                "Synthetic intercom".to_owned(),
                SecretString::from("v=0\r\na=synthetic\r\n"),
                SecretString::from("synthetic-intercom"),
                SecretString::from("synthetic-call-session"),
                now,
            ))
            .await
            .unwrap();

        let cancellation = CancellationToken::new();
        let task = tokio::spawn(run_auto_open_watcher(
            Arc::clone(&auto_open),
            Arc::clone(&calls),
            cancellation.clone(),
            Duration::from_millis(1),
        ));
        wait_until(|| *opener.calls.lock().unwrap() == 1).await;
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if auto_open.policy().await.unwrap().last_result.is_some() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(Duration::from_millis(15)).await;
        cancellation.cancel();
        task.await.unwrap();

        assert_eq!(*opener.calls.lock().unwrap(), 1);
        assert_eq!(transport.ignored.lock().unwrap().len(), expected_ignored);
        assert_eq!(
            auto_open
                .policy()
                .await
                .unwrap()
                .last_result
                .unwrap()
                .status,
            expected_status
        );
    }
}

async fn response_json(response: axum::response::Response) -> Value {
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

fn json_request(method: &str, uri: String, value: Value) -> Request<Body> {
    Request::builder()
        .method(method)
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&value).unwrap()))
        .unwrap()
}

#[tokio::test]
async fn api_requires_confirm_then_exposes_only_safe_policy_and_disables_immediately() {
    let dir = tempfile::tempdir().unwrap();
    let now = Utc.with_ymd_and_hms(2026, 8, 7, 16, 0, 0).unwrap();
    let clock = AdjustableClock::new(now);
    let store = SecretStore::open(dir.path(), Some([45_u8; 32]))
        .await
        .unwrap();
    store.save_session(&session(now)).await.unwrap();
    let auto_open = make_service(store, clock).await;
    let state = AppState::default().with_auto_open_service(auto_open);
    let config = AppConfig::from_iter(std::iter::empty::<(&str, &str)>()).unwrap();
    let app: Router = build_router_with_config(state, &config);

    let response = app
        .clone()
        .oneshot(
            Request::get("/api/v1/calls/auto-open")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
    assert_eq!(response_json(response).await["mode"], "disabled");

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            "/api/v1/calls/auto-open/intents".to_owned(),
            json!({"mode": "until", "dismissAfterOpen": true}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let intent = response_json(response).await;
    let intent_id = intent["intentId"].as_str().unwrap();

    let response = app
        .clone()
        .oneshot(json_request(
            "POST",
            format!("/api/v1/calls/auto-open/intents/{intent_id}/execute"),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let policy = response_json(response).await;
    assert_eq!(policy["mode"], "until");
    assert_eq!(policy["dismissAfterOpen"], true);
    assert_eq!(policy["until"], "2026-08-07T16:30:00Z");
    let encoded = serde_json::to_string(&policy).unwrap();
    assert!(!encoded.contains(intent_id));
    assert!(!encoded.contains("test-access"));

    let replay = app
        .clone()
        .oneshot(json_request(
            "POST",
            format!("/api/v1/calls/auto-open/intents/{intent_id}/execute"),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(replay.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        response_json(replay).await["code"],
        "auto_open_intent_not_found"
    );

    let disabled = app
        .oneshot(
            Request::delete("/api/v1/calls/auto-open")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(disabled.status(), StatusCode::NO_CONTENT);
    assert!(!dir.path().join("auto-open.enc").exists());
}
