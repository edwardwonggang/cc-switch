# Codex 接管恢复中的凭证边界

本规则适用于本地代理接管的启动崩溃恢复、正常退出清理和停用接管。普通供应商切换继续使用原有写入策略，不把 API Key 全局识别为 OAuth。

## 两类凭证分别归谁

- `auth.json` 是 Codex 客户端的当前登录状态。它可能是 OAuth、PAT 或只有 `OPENAI_API_KEY`，可以独立于 CC Switch 当前供应商存在。
- 供应商数据库 `settings_config.auth` 是该供应商的上游凭证来源。接管中的请求从选中的供应商读取它，不应使用另一来源的客户端 Key。
- 备份中的路由需要其自身凭证；保护 Live 登录时，原有 `prepare_codex_provider_live_config` 把备份路由凭证放到 `experimental_bearer_token`，不会把客户端 Key 注入该路由。

只比较 `auth.json` 是否保留不够：还必须核对恢复后重新接管的同步步骤，以及最终上游收到的凭证。

## 恢复与重新接管

启动先执行 `recover_from_crash`，恢复 Live 文件并清理旧备份，再按 `proxy_config.enabled` 恢复接管。

恢复时，当前非空且非 `PROXY_MANAGED` 的 API Key 与现有真实登录凭据共用快照保护步骤；不覆盖 `auth.json`，仍恢复备份中的供应商路由和模型目录。占位符、空 Key 和纯元数据不算当前凭证。缺失文件继续遵守原策略：普通第三方供应商可恢复备份；官方供应商或来源未知时保留登出状态。

重新接管前的同步必须遵守：

1. Live 路由与当前供应商的上游地址都明确存在且非空，再比较是否一致，才允许同步；忽略两端空白和地址末尾斜线。双方都缺失或为空不代表来源一致。
2. 存在 `experimental_bearer_token` 时优先同步该路由凭证，不能取独立客户端 `auth.json` 中的 Key。
3. 代理占位符不能回写数据库；官方账号卡和统一卡继续不接收此同步。

这样可以保留“Live 使用另一来源 Key、当前供应商仍为 Flow”的状态，而不把 Flow 的凭证改成客户端 Key。

## 隔离回归

测试使用临时 HOME、内存数据库和回环假上游，不调用真实供应商：

```sh
cargo test --manifest-path src-tauri/Cargo.toml recover_from_crash_
cargo test --manifest-path src-tauri/Cargo.toml codex_sync_live_uses_route_credentials_without_crossing_sources
cargo test --manifest-path src-tauri/Cargo.toml recover_and_retakeover_codex_preserves_client_auth_and_upstream_credentials
```

在 GitHub CI 执行 Rust 编译和测试。本机格式检查不能替代运行结果。完整 CI 还应覆盖既有 OAuth、登出、占位符、模型目录、事务回滚、普通供应商切换及统一卡路由测试。

## 替换与回退

1. 等待工作区空闲，保存实际路径中的设置、Live 文件、旧 EXE 和整个数据库的一致性备份，记录 SHA256、mtime 及原本不存在的文件。
2. 旧 EXE 退出时也会执行旧恢复链，可能先覆盖 Live 登录。待旧进程完全退出后，核对并从保护快照恢复被改动的 Live 登录，再启动候选程序；只在启动后恢复 Key 不能证明恢复链安全。
3. 按最终交付清单校验候选 EXE，在原安装路径替换。保持当前供应商、成员选择及配置目录不变。
4. 启动后读回 EXE 哈希、auth 字节、供应商配置、当前项和接管端口；再验证真实代理请求及网页 Win Codex 专用验收会话。
5. 失败时先停止候选进程，再恢复同一代 EXE、数据库及文件快照。旧版本仍有原恢复缺陷，回退后的启动也必须由同一守护流程核对 Live 登录；回退 EXE 本身不能证明凭证已恢复。

不得直接重放旧安装脚本：它包含超时强制结束进程，且首次安装清单只对应当时的程序 SHA。切换统一卡属于额外原生验收步骤，不能用安装成功或 `/v1/models` 成功代替。
