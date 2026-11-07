//! 固定 Codex 统一供应商的成员、别名和目录投影。
//! 调用方：供应商列表/配置命令、ProviderRouter、ProxyService/live 写入。
//! 依赖：数据库真实供应商、原 Codex 目录生成器和各成员协议能力。
//! 红线：只保存成员 ID；别名优先完整映射，旧模型名仅匹配唯一声明来源，不能拆字符串或默认降级；
//! 请求只能选映射中的真实成员，不能向上游发送别名或改写全局 current。
use crate::{
    app_config::AppType,
    database::Database,
    error::AppError,
    provider::{Provider, ProviderMeta},
};
use indexmap::IndexMap;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
pub const UNIFIED_PROVIDER_ID: &str = "codex-unified";
pub const UNIFIED_PROVIDER_TYPE: &str = "codex_unified";
// Codex 按 model_provider 对历史分桶；统一卡必须沿用普通供应商的 custom，
// 否则切卡后既有历史会不可见。此标识不改变 CC Switch 的固定卡数据库 ID。
const CODEX_LIVE_PROVIDER_ID: &str = "custom";
pub fn builtin() -> Provider {
    let mut provider = Provider::with_id(
        UNIFIED_PROVIDER_ID.to_string(),
        "Codex".to_string(),
        json!({"memberProviderIds": []}),
        None,
    );
    provider.category = Some("custom".to_string());
    provider.meta = Some(ProviderMeta {
        provider_type: Some(UNIFIED_PROVIDER_TYPE.to_string()),
        common_config_enabled: Some(true),
        ..Default::default()
    });
    provider
}
pub fn is_unified(provider: &Provider) -> bool {
    provider.id == UNIFIED_PROVIDER_ID
        || provider
            .meta
            .as_ref()
            .and_then(|meta| meta.provider_type.as_deref())
            == Some(UNIFIED_PROVIDER_TYPE)
}
pub fn active(db: &Database) -> Result<Option<Provider>, AppError> {
    let Some(id) = crate::settings::get_effective_current_provider(db, &AppType::Codex)? else {
        return Ok(None);
    };
    Ok(db.get_provider_by_id(&id, "codex")?.filter(is_unified))
}
pub fn eligible(provider: &Provider) -> bool {
    !is_unified(provider) && !super::providers::is_codex_official_provider(provider)
}
pub fn member_ids(provider: &Provider) -> Result<Vec<String>, AppError> {
    if provider.id != UNIFIED_PROVIDER_ID || provider.name != "Codex" {
        return Err(AppError::InvalidInput(
            "内置统一供应商的 ID 和名称不可修改".to_string(),
        ));
    }
    let rows = provider
        .settings_config
        .get("memberProviderIds")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::InvalidInput("统一供应商缺少成员列表".to_string()))?;
    let mut seen = HashSet::new();
    rows.iter()
        .map(|value| {
            let id = value
                .as_str()
                .filter(|id| !id.is_empty())
                .ok_or_else(|| AppError::InvalidInput("统一供应商成员 ID 无效".to_string()))?;
            if id == UNIFIED_PROVIDER_ID || !seen.insert(id.to_string()) {
                return Err(AppError::InvalidInput(
                    "统一供应商不能包含自身或重复成员".to_string(),
                ));
            }
            Ok(id.to_string())
        })
        .collect()
}
pub fn declared_models(provider: &Provider) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut models: Vec<String> = provider
        .settings_config
        .pointer("/modelCatalog/models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| row.get("model").and_then(Value::as_str))
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .filter(|model| seen.insert((*model).to_string()))
        .map(str::to_string)
        .collect();
    if models.is_empty() {
        models.extend(super::providers::codex_provider_upstream_model(provider));
    }
    models
}
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UnifiedModelPreview {
    pub alias: String,
    pub provider_id: String,
    pub provider_name: String,
    pub actual_model: String,
}
pub struct UnifiedRoute {
    pub preview: UnifiedModelPreview,
    pub provider: Provider,
}
fn friendly_prefix(name: &str) -> String {
    let prefix = name.split_whitespace().collect::<Vec<_>>().join("-");
    let prefix: String = prefix.chars().filter(|ch| !ch.is_control()).collect();
    if prefix.is_empty() {
        "provider".to_string()
    } else {
        prefix
    }
}
fn identity_suffix(id: &str) -> String {
    // Explicit FNV-1a is stable across processes/platforms/toolchain versions.
    let hash = id
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("{hash:016x}")
}
fn routes_from_providers(
    unified: &Provider,
    providers: &IndexMap<String, Provider>,
) -> Result<Vec<UnifiedRoute>, AppError> {
    let mut members = Vec::new();
    for id in member_ids(unified)? {
        let member = providers
            .get(&id)
            .ok_or_else(|| AppError::InvalidInput(format!("统一供应商成员已不存在: {id}")))?;
        if !eligible(member) {
            return Err(AppError::InvalidInput(format!(
                "{} 不能作为统一供应商成员",
                member.name
            )));
        }
        members.push((
            member.clone(),
            declared_models(member),
            friendly_prefix(&member.name),
            identity_suffix(&member.id),
        ));
    }
    let mut suffix_lengths = vec![0usize; members.len()];
    loop {
        let mut owners: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, (_, models, name, suffix)) in members.iter().enumerate() {
            let prefix = if suffix_lengths[index] == 0 {
                name.clone()
            } else {
                format!("{name}-{}", &suffix[..suffix_lengths[index]])
            };
            for model in models {
                owners
                    .entry(format!("{prefix}-{model}"))
                    .or_default()
                    .push(index);
            }
        }
        let conflicts: HashSet<usize> = owners
            .values()
            .filter(|owners| owners.len() > 1)
            .flatten()
            .copied()
            .collect();
        if conflicts.is_empty() {
            break;
        }
        // Suffix every conflicting member: changing member order must never
        // silently reassign an existing alias to a different source.
        for index in conflicts {
            if suffix_lengths[index] == 16 {
                return Err(AppError::InvalidInput(
                    "供应商模型别名存在无法消解的身份冲突".to_string(),
                ));
            }
            suffix_lengths[index] = if suffix_lengths[index] == 0 {
                6
            } else {
                suffix_lengths[index] + 2
            };
        }
    }
    let mut routes = Vec::new();
    for (index, (provider, models, name, suffix)) in members.into_iter().enumerate() {
        let prefix = if suffix_lengths[index] == 0 {
            name
        } else {
            format!("{name}-{}", &suffix[..suffix_lengths[index]])
        };
        for model in models {
            routes.push(UnifiedRoute {
                preview: UnifiedModelPreview {
                    alias: format!("{prefix}-{model}"),
                    provider_id: provider.id.clone(),
                    provider_name: provider.name.clone(),
                    actual_model: model,
                },
                provider: provider.clone(),
            });
        }
    }
    Ok(routes)
}
pub fn routes(db: &Database, unified: &Provider) -> Result<Vec<UnifiedRoute>, AppError> {
    routes_from_providers(unified, &db.get_all_providers("codex")?)
}
pub fn resolve(db: &Database, unified: &Provider, alias: &str) -> Result<UnifiedRoute, AppError> {
    let mut routes = routes(db, unified)?;
    if let Some(index) = routes.iter().position(|route| route.preview.alias == alias) {
        return Ok(routes.swap_remove(index));
    }
    // Codex 切换模型前会用上一回合模型压缩历史；重启客户端也不会改写历史中的裸模型名。
    // 只接受当前成员完整声明且唯一的真实模型，不能猜来源或把旧压缩改发到当前新模型。
    let mut legacy_matches = routes
        .into_iter()
        .filter(|route| route.preview.actual_model == alias);
    let route = legacy_matches
        .next()
        .ok_or_else(|| AppError::InvalidInput(format!("统一供应商中不存在模型别名: {alias}")))?;
    if legacy_matches.next().is_some() {
        return Err(AppError::InvalidInput(format!(
            "统一供应商中的模型名称存在多个来源，请使用完整模型别名: {alias}"
        )));
    }
    Ok(route)
}
pub fn validate_change(db: &Database, candidate: &Provider) -> Result<(), AppError> {
    if is_unified(candidate) {
        member_ids(candidate)?;
    }
    let mut providers = db.get_all_providers("codex")?;
    providers.insert(candidate.id.clone(), candidate.clone());
    let current = crate::settings::get_effective_current_provider(db, &AppType::Codex)?;
    for unified in providers.values().filter(|provider| is_unified(provider)) {
        let routes = routes_from_providers(unified, &providers)?;
        if current.as_deref() == Some(unified.id.as_str()) && routes.is_empty() {
            return Err(AppError::InvalidInput(
                "正在使用 Codex，请至少保留一个具有模型的供应商；如需清空，请先手动切回普通供应商"
                    .to_string(),
            ));
        }
    }
    Ok(())
}
pub fn catalog(db: &Database, unified: &Provider) -> Result<Value, AppError> {
    let routes = routes(db, unified)?;
    let mut source_catalogs = HashMap::new();
    let mut entries = Vec::new();
    for route in routes {
        if let std::collections::hash_map::Entry::Vacant(slot) =
            source_catalogs.entry(route.provider.id.clone())
        {
            let mut settings = route.provider.settings_config.clone();
            if settings
                .pointer("/modelCatalog/models")
                .and_then(Value::as_array)
                .is_none_or(Vec::is_empty)
            {
                settings["modelCatalog"] = json!({"models": declared_models(&route.provider).into_iter().map(|model| json!({"model": model})).collect::<Vec<_>>()});
            }
            let source = crate::codex_config::build_codex_model_catalog_for_provider(
                &route.provider,
                &settings,
            )?
            .unwrap_or_else(|| json!({"models": []}));
            slot.insert(source);
        }
        let entry = source_catalogs[&route.provider.id]
            .get("models")
            .and_then(Value::as_array)
            .and_then(|models| {
                models.iter().find(|entry| {
                    entry.get("slug").and_then(Value::as_str)
                        == Some(route.preview.actual_model.as_str())
                })
            })
            .ok_or_else(|| {
                AppError::Config(format!(
                    "{} 的模型目录缺少 {}",
                    route.provider.name, route.preview.actual_model
                ))
            })?;
        let mut entry = entry.clone();
        entry["slug"] = json!(route.preview.alias);
        entry["display_name"] = json!(route.preview.alias);
        entry["priority"] = json!(1000 + entries.len());
        entries.push(entry);
    }
    Ok(json!({"models": entries}))
}
pub fn live_settings(db: &Database, unified: &Provider) -> Result<Value, AppError> {
    let routes = routes(db, unified)?;
    let first = routes
        .first()
        .ok_or_else(|| AppError::InvalidInput("请至少勾选一个具有模型的供应商".to_string()))?;
    let model = toml::Value::String(first.preview.alias.clone()).to_string();
    let global = db.get_global_proxy_config_sync()?;
    let config = format!("model_provider = \"{provider}\"\nmodel = {model}\nmodel_catalog_json = \"cc-switch-model-catalog.json\"\n\n[model_providers.{provider}]\nname = \"Codex\"\nbase_url = \"http://127.0.0.1:{port}/v1\"\nwire_api = \"responses\"\nrequires_openai_auth = false\nexperimental_bearer_token = \"PROXY_MANAGED\"\n", provider = CODEX_LIVE_PROVIDER_ID, port = global.listen_port);
    Ok(
        json!({"auth": {"OPENAI_API_KEY": "PROXY_MANAGED"}, "config": config, "unifiedModelCatalog": catalog(db, unified)?}),
    )
}
