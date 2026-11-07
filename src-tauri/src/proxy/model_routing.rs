//! Codex 按模型选择供应商和聚合目录。
//! 调用方：ProviderRouter、代理 HTTP models 入口、ProxyService 配置投影。
//! 依赖：数据库供应商顺序、现有 Codex 目录生成器和供应商协议元数据。
//! 红线：精确且区分大小写；同模型才可互为候选；不改变全局当前供应商，
//! 因为并发的不同模型不能互相覆盖对方的凭证、目录或默认模型。

use crate::database::Database;
use crate::error::AppError;
use crate::provider::Provider;
use serde_json::{json, Value};
use std::collections::HashSet;

pub const CODEX_MODEL_ROUTING_SETTING: &str = "codex_model_routing_enabled";

pub fn enabled(db: &Database) -> Result<bool, AppError> {
    db.get_bool_flag(CODEX_MODEL_ROUTING_SETTING)
}

pub fn eligible(provider: &Provider) -> bool {
    // Native official requests carry the selected account's token. It cannot
    // be reused for an unrelated card as an ordinary model-routing candidate.
    !super::providers::is_codex_official_provider(provider)
}

pub fn declared_models(provider: &Provider) -> Vec<String> {
    let models: Vec<String> = provider
        .settings_config
        .pointer("/modelCatalog/models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("model").and_then(Value::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
        .collect();
    if models.is_empty() {
        super::providers::codex_provider_upstream_model(provider)
            .into_iter()
            .collect()
    } else {
        models
    }
}

pub fn supports_model(provider: &Provider, model: &str) -> bool {
    eligible(provider) && declared_models(provider).iter().any(|id| id == model)
}

pub fn combined_catalog(db: &Database) -> Result<Value, AppError> {
    let providers = db.get_all_providers("codex")?;
    let mut seen = HashSet::new();
    let mut models = Vec::new();
    // DAO order is also the route priority: sort_index, created_at, id. Keep
    // the first provider's entire generated entry for duplicate model names;
    // using one global tool profile would break mixed Chat/Responses gateways.
    for provider in providers.values().filter(|provider| eligible(provider)) {
        let mut settings = provider.settings_config.clone();
        if declared_models(provider).is_empty() {
            continue;
        }
        if settings
            .pointer("/modelCatalog/models")
            .and_then(Value::as_array)
            .is_none_or(Vec::is_empty)
        {
            settings["modelCatalog"] = json!({
                "models": declared_models(provider)
                    .into_iter()
                    .map(|model| json!({"model": model}))
                    .collect::<Vec<_>>()
            });
        }
        let Some(catalog) =
            crate::codex_config::build_codex_model_catalog_for_provider(provider, &settings)?
        else {
            continue;
        };
        for mut entry in catalog
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .cloned()
        {
            let Some(slug) = entry.get("slug").and_then(Value::as_str) else {
                continue;
            };
            if seen.insert(slug.to_string()) {
                entry["priority"] = json!(1000 + models.len());
                models.push(entry);
            }
        }
    }
    Ok(json!({ "models": models }))
}
