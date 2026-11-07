//! 隔离的 HTTP→供应商→模型→响应回归，真实用户库与配置均不参与。
use super::{model_routing, server::ProxyServer, ProxyConfig};
use crate::{
    database::Database,
    provider::{Provider, ProviderMeta},
};
use axum::{
    extract::Request,
    http::{header, StatusCode},
    routing::post,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::Mutex;

struct TestHome {
    _dir: tempfile::TempDir,
    previous: Option<std::ffi::OsString>,
}
impl TestHome {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let previous = std::env::var_os("CC_SWITCH_TEST_HOME");
        std::env::set_var("CC_SWITCH_TEST_HOME", dir.path());
        crate::settings::reload_settings().unwrap();
        assert!(crate::codex_config::get_codex_config_dir().starts_with(dir.path()));
        Self {
            _dir: dir,
            previous,
        }
    }
}
impl Drop for TestHome {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.as_ref() {
            std::env::set_var("CC_SWITCH_TEST_HOME", previous);
        } else {
            std::env::remove_var("CC_SWITCH_TEST_HOME");
        }
        crate::settings::reload_settings().unwrap();
    }
}

#[tokio::test]
#[serial_test::serial]
async fn model_routing_http_isolates_models_credentials_failover_and_default() {
    let _home = TestHome::new();
    let db = Arc::new(Database::memory().unwrap());
    let captured = Arc::new(Mutex::new(Vec::<(String, String, String)>::new()));
    let fail_first = Arc::new(AtomicBool::new(false));
    let mut mocks = Vec::new();
    for (id, format, models, sort) in [
        ("a", "openai_responses", vec!["Model-A", "Shared"], 0),
        ("b", "openai_chat", vec!["Model-B", "Shared"], 1),
        ("unrelated", "openai_responses", vec!["Other"], 2),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let recorded = captured.clone();
        let fail = fail_first.clone();
        let handler = move |request: Request| {
            let recorded = recorded.clone();
            let fail = fail.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = axum::body::to_bytes(body, 1024 * 1024).await.unwrap();
                let body: Value = serde_json::from_slice(&bytes).unwrap();
                let model = body["model"].as_str().unwrap().to_string();
                let auth = parts
                    .headers
                    .get(header::AUTHORIZATION)
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .to_string();
                recorded
                    .lock()
                    .await
                    .push((id.to_string(), model.clone(), auth));
                if id == "a" && model == "Shared" && fail.load(Ordering::SeqCst) {
                    return (
                        StatusCode::BAD_GATEWAY,
                        Json(json!({"error": {"message": "fake A unavailable"}})),
                    );
                }
                let response = if format == "openai_chat" {
                    json!({"id": "chatcmpl-b", "object": "chat.completion", "model": model,
                           "choices": [{"index": 0, "message": {"role": "assistant", "content": id}, "finish_reason": "stop"}],
                           "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}})
                } else {
                    json!({"id": format!("resp-{id}"), "object": "response", "status": "completed", "model": model,
                           "output": [{"id": "msg-1", "type": "message", "role": "assistant", "status": "completed",
                                       "content": [{"type": "output_text", "text": id, "annotations": []}]}],
                           "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}})
                };
                (StatusCode::OK, Json(response))
            }
        };
        let app = Router::new()
            .route("/v1/responses", post(handler.clone()))
            .route("/v1/chat/completions", post(handler));
        mocks.push(tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap()
        }));
        let mut provider = Provider::with_id(
            id.to_string(),
            id.to_string(),
            json!({"base_url": format!("http://{address}/v1"),
                   "auth": {"OPENAI_API_KEY": format!("fake-{id}")},
                   "model": format!("default-{id}"),
                   "modelCatalog": {"models": models.into_iter().map(|model| json!({"model": model})).collect::<Vec<_>>()}}),
            None,
        );
        provider.sort_index = Some(sort);
        provider.meta = Some(ProviderMeta {
            api_format: Some(format.to_string()),
            ..Default::default()
        });
        db.save_provider("codex", &provider).unwrap();
    }
    db.set_current_provider("codex", "a").unwrap();
    crate::settings::set_current_provider(&crate::app_config::AppType::Codex, Some("a")).unwrap();
    db.set_setting(model_routing::CODEX_MODEL_ROUTING_SETTING, "true")
        .unwrap();
    let mut config = db.get_proxy_config_for_app("codex").await.unwrap();
    config.auto_failover_enabled = true;
    config.max_retries = 3;
    db.update_proxy_config_for_app(config).await.unwrap();
    db.add_to_failover_queue("codex", "a").unwrap();
    let proxy = ProxyServer::new(
        ProxyConfig {
            listen_port: 0,
            enable_logging: false,
            ..Default::default()
        },
        db.clone(),
        None,
    );
    let info = proxy.start().await.unwrap();
    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let endpoint = format!("http://127.0.0.1:{}/v1/responses", info.port);
    let request = |model: &str| {
        client
            .post(&endpoint)
            .header(header::AUTHORIZATION, "Bearer fake-client")
            .json(&json!({"model": model, "input": "hello", "stream": false}))
    };
    let (a, b) = tokio::join!(request("Model-A").send(), request("Model-B").send());
    assert_eq!(a.unwrap().status(), StatusCode::OK);
    assert_eq!(b.unwrap().status(), StatusCode::OK);
    assert_eq!(
        request("Shared").send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(captured.lock().await.last().unwrap().0, "a");
    fail_first.store(true, Ordering::SeqCst);
    assert_eq!(
        request("Shared").send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(captured.lock().await.last().unwrap().0, "b");
    assert_eq!(
        request("unknown").send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(captured.lock().await.last().unwrap().0, "a");
    db.set_setting(model_routing::CODEX_MODEL_ROUTING_SETTING, "false")
        .unwrap();
    assert_eq!(
        request("Model-B").send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(captured.lock().await.last().unwrap().0, "a");
    let records = captured.lock().await;
    assert!(records
        .iter()
        .any(|(id, model, auth)| id == "b" && model == "Model-B" && auth == "Bearer fake-b"));
    assert!(records
        .iter()
        .any(|(id, model, auth)| id == "a" && model == "Model-A" && auth == "Bearer fake-a"));
    assert!(!records.iter().any(|(id, _, _)| id == "unrelated"));
    assert_eq!(
        db.get_current_provider("codex").unwrap().as_deref(),
        Some("a")
    );
    assert_eq!(proxy.get_status().await.failover_count, 0);
    drop(records);
    proxy.stop().await.unwrap();
    for handle in mocks {
        handle.abort();
    }
}
