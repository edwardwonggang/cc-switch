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

#[test]
#[serial_test::serial]
fn unified_builtin_is_always_listed_without_persisting_or_selecting_it() {
    let _home = TestHome::new();
    let db = Arc::new(Database::memory().unwrap());
    let state = crate::store::AppState::new(db.clone());
    for _ in 0..2 {
        let listed =
            crate::services::ProviderService::list(&state, crate::app_config::AppType::Codex)
                .unwrap();
        assert_eq!(
            listed.first().unwrap().0,
            model_routing::UNIFIED_PROVIDER_ID
        );
        assert_eq!(listed[model_routing::UNIFIED_PROVIDER_ID].name, "Codex");
        assert!(db.get_all_providers("codex").unwrap().is_empty());
        assert!(db.get_current_provider("codex").unwrap().is_none());
    }
    assert!(
        crate::services::ProviderService::list(&state, crate::app_config::AppType::Claude,)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
#[serial_test::serial]
async fn unified_member_save_is_db_only_until_selected_and_rejects_empty_active_card() {
    let _home = TestHome::new();
    let db = Arc::new(Database::memory().unwrap());
    let state = crate::store::AppState::new(db.clone());
    let config_path = crate::codex_config::get_codex_config_path();
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    std::fs::write(&config_path, "model = \"unchanged-user-choice\"\n").unwrap();
    let original_config = std::fs::read(&config_path).unwrap();
    let member = Provider::with_id(
        "a".to_string(),
        "A".to_string(),
        json!({"auth": {}, "config": "model = \"real-model\"\n",
               "modelCatalog": {"models": [{"model": "real-model"}]}}),
        None,
    );
    db.save_provider("codex", &member).unwrap();
    let unified = state
        .proxy_service
        .save_codex_unified_members(vec!["a".to_string()])
        .await
        .unwrap();
    assert_eq!(model_routing::member_ids(&unified).unwrap(), ["a"]);
    assert!(db.get_current_provider("codex").unwrap().is_none());
    assert_eq!(std::fs::read(&config_path).unwrap(), original_config);
    assert_eq!(
        db.get_provider_by_id("a", "codex")
            .unwrap()
            .unwrap()
            .settings_config,
        member.settings_config
    );
    assert!(db.get_live_backup("codex").await.unwrap().is_none());
    assert!(!state.proxy_service.is_running().await);

    db.set_current_provider("codex", &unified.id).unwrap();
    crate::settings::set_current_provider(&crate::app_config::AppType::Codex, Some(&unified.id))
        .unwrap();
    assert!(state
        .proxy_service
        .save_codex_unified_members(Vec::new())
        .await
        .is_err());
    assert_eq!(
        model_routing::member_ids(
            &db.get_provider_by_id(&unified.id, "codex")
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        ["a"]
    );
    assert_eq!(
        db.get_current_provider("codex").unwrap().as_deref(),
        Some(unified.id.as_str())
    );
    assert_eq!(std::fs::read(&config_path).unwrap(), original_config);
    assert!(crate::services::ProviderService::delete(
        &state,
        crate::app_config::AppType::Codex,
        "a"
    )
    .is_err());
    assert!(crate::services::ProviderService::delete(
        &state,
        crate::app_config::AppType::Codex,
        &unified.id
    )
    .is_err());
    assert!(db.get_provider_by_id("a", "codex").unwrap().is_some());

    // An invalid ordinary-provider edit must fail before changing its name,
    // credential source or the unified membership.
    let mut invalid = member.clone();
    invalid.name = "B".to_string();
    invalid.settings_config["auth"] = json!("invalid-auth-shape");
    assert!(crate::services::ProviderService::update(
        &state,
        crate::app_config::AppType::Codex,
        None,
        invalid,
    )
    .is_err());
    assert_eq!(
        db.get_provider_by_id("a", "codex").unwrap().unwrap().name,
        "A"
    );
    db.conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER reject_unified_backup BEFORE INSERT ON proxy_live_backup
         BEGIN SELECT RAISE(ABORT, 'fake backup blocked'); END;",
        )
        .unwrap();
    let mut renamed = member.clone();
    renamed.name = "Renamed A".to_string();
    let error = crate::services::ProviderService::update(
        &state,
        crate::app_config::AppType::Codex,
        None,
        renamed,
    )
    .expect_err("the injected backup failure must abort the member edit");
    assert!(
        error.to_string().contains("fake backup blocked"),
        "the valid rename must reach the failing backup commit, got: {error}"
    );
    assert_eq!(
        db.get_provider_by_id("a", "codex").unwrap().unwrap().name,
        "A"
    );
    assert_eq!(std::fs::read(&config_path).unwrap(), original_config);
    assert!(!crate::codex_config::get_codex_model_catalog_path().exists());
    assert_eq!(
        db.get_current_provider("codex").unwrap().as_deref(),
        Some(unified.id.as_str())
    );
}

#[test]
#[serial_test::serial]
fn unified_alias_map_handles_hyphens_renames_missing_and_recursive_members() {
    let _home = TestHome::new();
    let db = Database::memory().unwrap();
    let a = Provider::with_id(
        "a".to_string(),
        "A-B".to_string(),
        json!({
            "modelCatalog": {"models": [{"model": "C"}]}
        }),
        None,
    );
    let b = Provider::with_id(
        "b".to_string(),
        "A".to_string(),
        json!({
            "modelCatalog": {"models": [{"model": "B-C"}]}
        }),
        None,
    );
    db.save_provider("codex", &a).unwrap();
    db.save_provider("codex", &b).unwrap();
    let mut unified = model_routing::builtin();
    unified.settings_config["memberProviderIds"] = json!(["a", "b"]);
    let routes = model_routing::routes(&db, &unified).unwrap();
    assert_eq!(routes.len(), 2);
    assert_ne!(routes[0].preview.alias, routes[1].preview.alias);
    for expected in routes {
        let resolved = model_routing::resolve(&db, &unified, &expected.preview.alias).unwrap();
        assert_eq!(resolved.provider.id, expected.preview.provider_id);
        assert_eq!(resolved.preview.actual_model, expected.preview.actual_model);
    }
    let old_alias = model_routing::routes(&db, &unified).unwrap()[0]
        .preview
        .alias
        .clone();
    let mut renamed = a.clone();
    renamed.name = "公司 New".to_string();
    db.save_provider("codex", &renamed).unwrap();
    assert!(model_routing::resolve(&db, &unified, &old_alias).is_err());
    let renamed_route = model_routing::resolve(&db, &unified, "公司-New-C").unwrap();
    assert_eq!(renamed_route.provider.id, "a");
    assert_eq!(renamed_route.preview.actual_model, "C");
    db.delete_provider("codex", "a").unwrap();
    assert!(model_routing::resolve(&db, &unified, "公司-New-C").is_err());
    unified.settings_config["memberProviderIds"] = json!([model_routing::UNIFIED_PROVIDER_ID]);
    assert!(model_routing::routes(&db, &unified).is_err());
}

#[test]
#[serial_test::serial]
fn unified_legacy_models_require_one_declared_member_and_explicit_alias_wins() {
    let _home = TestHome::new();
    let db = Database::memory().unwrap();
    for (id, name, models) in [
        ("a", "A", vec!["Model-A", "Shared"]),
        ("b", "B", vec!["Model-B", "Shared", "A-Model-A"]),
        ("unrelated", "Unrelated", vec!["Other"]),
    ] {
        let provider = Provider::with_id(
            id.to_string(),
            name.to_string(),
            json!({
                "auth": {"OPENAI_API_KEY": format!("fixture-{id}")},
                "modelCatalog": {"models": models.into_iter().map(|model| json!({"model": model})).collect::<Vec<_>>()}
            }),
            None,
        );
        db.save_provider("codex", &provider).unwrap();
    }
    let mut unified = model_routing::builtin();
    unified.settings_config["memberProviderIds"] = json!(["a", "b"]);
    let catalog_before = model_routing::catalog(&db, &unified).unwrap();
    for (model, source) in [("Model-A", "a"), ("Model-B", "b")] {
        let route = model_routing::resolve(&db, &unified, model).unwrap();
        assert_eq!(route.provider.id, source);
        assert_eq!(route.preview.actual_model, model);
        assert_eq!(
            route.provider.settings_config["auth"]["OPENAI_API_KEY"],
            format!("fixture-{source}")
        );
    }
    // B 声明的真实模型恰好等于 A 的完整别名时，显式别名仍指向 A。
    let explicit = model_routing::resolve(&db, &unified, "A-Model-A").unwrap();
    assert_eq!(explicit.provider.id, "a");
    assert_eq!(explicit.preview.actual_model, "Model-A");
    let shared = model_routing::resolve(&db, &unified, "Shared")
        .err()
        .expect("同名多来源必须拒绝");
    assert!(shared.to_string().contains("多个来源"));
    for rejected in ["unknown", "Other", "model-a", " Model-A ", "A-removed"] {
        assert!(model_routing::resolve(&db, &unified, rejected).is_err());
    }
    unified.settings_config["memberProviderIds"] = json!(["a"]);
    assert!(model_routing::resolve(&db, &unified, "Model-B").is_err());
    unified.settings_config["memberProviderIds"] = json!(["a", "b"]);
    assert_eq!(
        model_routing::catalog(&db, &unified).unwrap(),
        catalog_before
    );
    assert!(db.get_current_provider("codex").unwrap().is_none());
}

#[test]
#[serial_test::serial]
fn unified_live_settings_preserve_custom_history_identity_and_alias_routing() {
    let _home = TestHome::new();
    let db = Database::memory().unwrap();
    let member = Provider::with_id(
        "flow".to_string(),
        "flow".to_string(),
        json!({
            "auth": {"OPENAI_API_KEY": "fixture-flow-key"},
            "modelCatalog": {"models": [{"model": "Flash"}, {"model": "Thinking"}]}
        }),
        None,
    );
    db.save_provider("codex", &member).unwrap();
    let mut unified = model_routing::builtin();
    unified.settings_config["memberProviderIds"] = json!(["flow"]);
    let settings = model_routing::live_settings(&db, &unified).unwrap();
    let config: toml::Value = settings["config"].as_str().unwrap().parse().unwrap();
    assert_eq!(config["model_provider"].as_str(), Some("custom"));
    let providers = config["model_providers"].as_table().unwrap();
    assert_eq!(providers.len(), 1);
    let provider = providers.get("custom").unwrap();
    assert_eq!(
        provider["base_url"].as_str(),
        Some(
            format!(
                "http://127.0.0.1:{}/v1",
                db.get_global_proxy_config_sync().unwrap().listen_port
            )
            .as_str()
        )
    );
    assert_eq!(provider["wire_api"].as_str(), Some("responses"));
    assert_eq!(provider["requires_openai_auth"].as_bool(), Some(false));
    assert_eq!(
        provider["experimental_bearer_token"].as_str(),
        Some("PROXY_MANAGED")
    );
    assert_eq!(settings["auth"]["OPENAI_API_KEY"], "PROXY_MANAGED");
    assert_eq!(
        config["model_catalog_json"].as_str(),
        Some("cc-switch-model-catalog.json")
    );
    assert_eq!(config["model"].as_str(), Some("flow-Flash"));
    let aliases: Vec<&str> = settings["unifiedModelCatalog"]["models"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["slug"].as_str().unwrap())
        .collect();
    assert_eq!(aliases, ["flow-Flash", "flow-Thinking"]);
    for (alias, actual) in [("flow-Flash", "Flash"), ("flow-Thinking", "Thinking")] {
        let route = model_routing::resolve(&db, &unified, alias).unwrap();
        assert_eq!(route.provider.id, member.id);
        assert_eq!(route.preview.actual_model, actual);
        assert_eq!(route.provider.settings_config, member.settings_config);
    }
    assert_eq!(unified.id, "codex-unified");
    assert!(db.get_current_provider("codex").unwrap().is_none());
    assert!(!crate::codex_config::get_codex_config_path().exists());
}

#[tokio::test]
#[serial_test::serial]
async fn unified_member_model_edits_and_member_saves_keep_custom_live_and_backup_identity() {
    let _home = TestHome::new();
    let db = Arc::new(Database::memory().unwrap());
    let state = crate::store::AppState::new(db.clone());
    let config_path = crate::codex_config::get_codex_config_path();
    let auth_path = crate::codex_config::get_codex_auth_path();
    std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
    let auth_before = br#"{"OPENAI_API_KEY":"fixture-client-login"}"#;
    std::fs::write(&auth_path, auth_before).unwrap();
    let tokenflux = Provider::with_id(
        "tokenflux".to_string(),
        "tokenflux".to_string(),
        json!({
            "auth": {"OPENAI_API_KEY": "fixture-tokenflux-key"},
            "config": "model_provider = \"custom\"\nmodel = \"zzzcod-gpt-6.1-sol\"\n",
            "modelCatalog": {"models": [{"model": "zzzcod-gpt-6.1-sol"}]}
        }),
        None,
    );
    let flow = Provider::with_id(
        "flow".to_string(),
        "flow".to_string(),
        json!({
            "auth": {"OPENAI_API_KEY": "fixture-flow-key"},
            "config": "model_provider = \"custom\"\nmodel = \"Flash\"\n",
            "modelCatalog": {"models": [{"model": "Flash"}]}
        }),
        None,
    );
    db.save_provider("codex", &tokenflux).unwrap();
    db.save_provider("codex", &flow).unwrap();
    let mut unified = model_routing::builtin();
    unified.settings_config["memberProviderIds"] = json!(["tokenflux"]);
    db.save_provider("codex", &unified).unwrap();
    db.set_current_provider("codex", &unified.id).unwrap();
    crate::settings::set_current_provider(&crate::app_config::AppType::Codex, Some(&unified.id))
        .unwrap();
    let assert_custom = |text: &str| {
        let parsed: toml::Value = text.parse().unwrap();
        assert_eq!(parsed["model_provider"].as_str(), Some("custom"));
        assert!(parsed["model_providers"]["custom"].is_table());
        assert!(parsed["model_providers"].get("cc_switch_unified").is_none());
    };
    for add_model in [false, true] {
        if add_model {
            // 模拟探测结果保存时追加目录项，不更换供应商Key或默认模型。
            let mut edited = tokenflux.clone();
            edited.settings_config["modelCatalog"]["models"]
                .as_array_mut()
                .unwrap()
                .push(json!({"model": "zzzcod-gpt-6-astra"}));
            crate::services::ProviderService::update(
                &state,
                crate::app_config::AppType::Codex,
                None,
                edited.clone(),
            )
            .unwrap();
            assert_eq!(
                db.get_provider_by_id("tokenflux", "codex")
                    .unwrap()
                    .unwrap()
                    .settings_config,
                edited.settings_config
            );
        } else {
            state
                .proxy_service
                .save_codex_unified_members(vec!["tokenflux".to_string(), "flow".to_string()])
                .await
                .unwrap();
        }
        assert_custom(&std::fs::read_to_string(&config_path).unwrap());
        let backup = db.get_live_backup("codex").await.unwrap().unwrap();
        let backup: Value = serde_json::from_str(&backup.original_config).unwrap();
        assert_custom(backup["config"].as_str().unwrap());
        let catalog: Value = serde_json::from_slice(
            &std::fs::read(crate::codex_config::get_codex_model_catalog_path()).unwrap(),
        )
        .unwrap();
        let aliases: Vec<&str> = catalog["models"]
            .as_array()
            .unwrap()
            .iter()
            .map(|model| model["slug"].as_str().unwrap())
            .collect();
        assert!(aliases.contains(&"tokenflux-zzzcod-gpt-6.1-sol"));
        assert!(aliases.contains(&"flow-Flash"));
        assert_eq!(aliases.contains(&"tokenflux-zzzcod-gpt-6-astra"), add_model);
        assert_eq!(
            std::fs::read(&auth_path).unwrap().as_slice(),
            &auth_before[..]
        );
        assert_eq!(
            db.get_provider_by_id("flow", "codex")
                .unwrap()
                .unwrap()
                .settings_config,
            flow.settings_config
        );
        assert_eq!(
            db.get_current_provider("codex").unwrap().as_deref(),
            Some(model_routing::UNIFIED_PROVIDER_ID)
        );
        assert_eq!(
            crate::settings::get_current_provider(&crate::app_config::AppType::Codex).as_deref(),
            Some(model_routing::UNIFIED_PROVIDER_ID)
        );
    }
}

#[tokio::test]
#[serial_test::serial]
async fn unified_provider_http_routes_aliases_to_exact_member_and_never_falls_back() {
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
                if id == "a" && fail.load(Ordering::SeqCst) {
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
            .route("/v1/responses/compact", post(handler.clone()))
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
    let mut unified = model_routing::builtin();
    unified.settings_config["memberProviderIds"] = json!(["a", "b"]);
    db.save_provider("codex", &unified).unwrap();
    db.set_current_provider("codex", &unified.id).unwrap();
    crate::settings::set_current_provider(&crate::app_config::AppType::Codex, Some(&unified.id))
        .unwrap();
    let mut config = db.get_proxy_config_for_app("codex").await.unwrap();
    config.auto_failover_enabled = true;
    config.max_retries = 3;
    db.update_proxy_config_for_app(config).await.unwrap();
    db.add_to_failover_queue("codex", "a").unwrap();
    db.add_to_failover_queue("codex", "b").unwrap();
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
    let compact_request = |model: &str| {
        client
            .post(format!("{endpoint}/compact"))
            .header(header::AUTHORIZATION, "Bearer fake-client")
            .json(&json!({"model": model, "input": "history fixture", "stream": false}))
    };
    let (a, b) = tokio::join!(request("a-Model-A").send(), request("b-Model-B").send());
    assert_eq!(a.unwrap().status(), StatusCode::OK);
    assert_eq!(b.unwrap().status(), StatusCode::OK);
    for (model, source) in [("Model-A", "a"), ("Model-B", "b")] {
        for compact in [false, true] {
            let response = if compact {
                compact_request(model).send().await.unwrap()
            } else {
                request(model).send().await.unwrap()
            };
            assert_eq!(response.status(), StatusCode::OK);
            let records = captured.lock().await;
            let (provider, actual, auth) = records.last().unwrap();
            assert_eq!(provider, source);
            assert_eq!(actual, model);
            assert_eq!(auth, &format!("Bearer fake-{source}"));
        }
    }
    assert_eq!(
        request("a-Shared").send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(captured.lock().await.last().unwrap().0, "a");
    assert_eq!(
        request("b-Shared").send().await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(captured.lock().await.last().unwrap().0, "b");
    fail_first.store(true, Ordering::SeqCst);
    let before_failure = captured.lock().await.len();
    assert_eq!(
        request("a-Shared").send().await.unwrap().status(),
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(captured.lock().await.len(), before_failure + 1);
    assert_eq!(captured.lock().await.last().unwrap().0, "a");
    let before_legacy_failure = captured.lock().await.len();
    assert_eq!(
        compact_request("Model-A").send().await.unwrap().status(),
        StatusCode::BAD_GATEWAY
    );
    assert_eq!(captured.lock().await.len(), before_legacy_failure + 1);
    assert_eq!(captured.lock().await.last().unwrap().0, "a");
    let before_unknown = captured.lock().await.len();
    assert_eq!(
        request("unknown").send().await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request("Shared").send().await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    for rejected in ["Shared", "Other", "unknown", "a-removed"] {
        assert_eq!(
            compact_request(rejected).send().await.unwrap().status(),
            StatusCode::BAD_REQUEST
        );
    }
    assert_eq!(
        request("Other").send().await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(captured.lock().await.len(), before_unknown);
    assert_eq!(
        db.get_current_provider("codex").unwrap().as_deref(),
        Some(unified.id.as_str())
    );
    // An ordinary provider explicitly selected by the user restores its exact
    // legacy routing behavior; the unified card remains available in the DB.
    db.set_current_provider("codex", "a").unwrap();
    crate::settings::set_current_provider(&crate::app_config::AppType::Codex, Some("a")).unwrap();
    fail_first.store(false, Ordering::SeqCst);
    assert_eq!(
        request("Model-A").send().await.unwrap().status(),
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
