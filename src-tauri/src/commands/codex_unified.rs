//! 内置 Codex 卡片的无密钥预览与成员配置。
//! 调用方：CodexUnifiedProviderCard；依赖统一供应商解析器与 ProxyService 保存事务。
//! 红线：只写固定卡的成员 ID，不创建真实供应商副本或修改真实成员配置。
use crate::{provider::Provider, proxy::model_routing, store::AppState};
use tauri::Manager;

#[tauri::command]
pub fn get_codex_unified_preview(
    state: tauri::State<'_, AppState>,
    member_provider_ids: Vec<String>,
) -> Result<Vec<model_routing::UnifiedModelPreview>, String> {
    let mut provider = model_routing::builtin();
    provider.settings_config["memberProviderIds"] = serde_json::json!(member_provider_ids);
    model_routing::routes(&state.db, &provider)
        .map(|routes| routes.into_iter().map(|route| route.preview).collect())
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn save_codex_unified_members(
    app_handle: tauri::AppHandle,
    member_provider_ids: Vec<String>,
) -> Result<Provider, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let state = app_handle
            .try_state::<AppState>()
            .ok_or_else(|| "应用状态不可用".to_string())?;
        futures::executor::block_on(
            state
                .proxy_service
                .save_codex_unified_members(member_provider_ids),
        )
    })
    .await
    .map_err(|error| format!("保存统一供应商失败: {error}"))?
}
