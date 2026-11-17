# Windows 便携包的构建与复用

Windows 程序由 GitHub runner 构建。用户电脑上的 Rust 安装不参与这些 CI 任务。
源码、依赖或编译输入变化时，需要生成对应的新 EXE；输入相同的任务可以复用已验证程序，
无需再次执行前端生成和 Rust 编译。用户无需改变客户端操作。

## 两种缓存分别保存什么

- Cargo 缓存保存编译依赖和中间目标。恢复成功后，Cargo 仍会检查源码和构建脚本的脏状态，
  因此“Cargo cache hit”不能证明跳过了应用本体编译。
- 最终 EXE 缓存保存 `cc-switch.exe` 和来源清单。只有完整构建输入指纹与 EXE 哈希均匹配，
  CI 才跳过依赖恢复安装、Vite 生成和 Rust 编译，直接包装同一程序。

现有 Vite 配置会清空并重新生成 `dist`，Rust 构建脚本也会调用 Tauri 构建逻辑。
每次 checkout 的文件时间与重新生成的嵌入资源可能令 Cargo 重新判定本体需要构建；
最终 EXE 的内容指纹复用不依赖这些时间戳。

## 输入指纹与原始出处

`.github/scripts/windows-portable-cache.ps1` 对以下输入记录并计算内容指纹：

- Rust、前端、资源、构建脚本、Tauri 配置、依赖锁及相关根配置文件。
- 实际 `rustc`、Node、pnpm 版本和 GitHub runner 镜像、架构。
- 与真实编译共用的 Tauri 参数列表，以及相关编译和前端环境变量的哈希。

Git SHA 不参与是否需要编译的判断。工作流展示、文档等变化可产生不同 Git SHA，
而应用构建输入保持相同；任一受控输入变化会使用不同缓存。

artifact 中额外保存 `*-build-provenance.json`：

| 字段                | 含义                       |
| ------------------- | -------------------------- |
| `packagedForSha`    | 本次包装所属提交           |
| `compiledSourceSha` | 实际编译产生 EXE 的提交    |
| `inputHash`         | 本次与原编译共用的输入指纹 |
| `inputsEquivalent`  | 输入是否匹配               |
| `executableSha256`  | 实际 EXE 的校验值          |
| `reusedExecutable`  | 本次是否复用已有程序       |

原编译 SHA 与包装 SHA 可以不同，但输入不一致时绝不能复用；
清单保留原始来源，不把旧程序描述为不同源码的新编译结果。

## 如何验证加速

先检查“Cache compiled portable executable”与“Build Tauri App”步骤。
真正命中时，构建日志应记录原编译 SHA 和等价输入，
依赖安装与 Cargo 目标恢复步骤应跳过，来源清单的 `reusedExecutable` 为 `true`。
同时对比首个编译产物与复用产物的 EXE SHA256，确认是同一个已验证程序。

首次建立缓存仍需编译，并增加缓存保存开销。比较耗时要同时记录命中状态、
runner 差异和各步骤时间，不能把首次迁移成本或机器波动换算为确定的提速比例。
完整前端和多平台后端 CI 保留；同一应用输入复用不意味着省略验证。

CI 默认只生成 portable，避免不需要的 WiX/NSIS 下载和安装器打包。
正式发布及原有安装器路径仍可生成 MSI/NSIS，Rust release 优化等级、LTO 和代码生成策略保持不变。
