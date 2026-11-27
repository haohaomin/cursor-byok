# PR #56：可配置的自动压缩超时

来源：[本 fork PR #56](https://github.com/haohaomin/cursor-byok/pull/56)，作者 ShowBoy7816，来源 head `b3af330db5412f5484d739b075870d7216921a62`。集成基线 `6f89c9929853b5da165d559bea1c0f42e6f3c6ad`；工作分支 `codex/pr56-compaction-settings`。

当前状态：原始七项提交已采用并补全，经 [PR #60](https://github.com/haohaomin/cursor-byok/pull/60) 合入 main；原 PR #56 也已自动标记合并。合并提交 `793817a90732808f0f7af97a8442aa7183bbe250`。已随 v1.2.9 发布并完成资产核验，v1.2.8 不包含此设置。

## 行为与范围

旧实现对每批自动摘要使用固定 30 秒时限，供应商仍在生成摘要时也可能被取消。新实现默认 180 秒，可在设置页修改为 30–3600 秒；下一次自动压缩读取已保存值，无需重启。该值控制每批摘要调用，不是普通 Agent 请求或后台 Shell 的时限，也不控制手动压缩。

```text
apps/desktop/src/features/settings/  # 设置输入、校验及保存中的禁用状态
             ↓ HTTP GET/PUT
server/src/control/                  # 本地管理 API
             ↓
server/src/store/settings.rs         # 既有 service_settings 表内保存 run_compaction
             ↓ 每次自动压缩开始时读取一次
server/src/run/engine/compaction.rs   # 每批摘要独立计时；完整摘要成功后提交
```

沿用既有表，不新增或改写迁移。正在执行的压缩保持启动时读取的时限；修改后的值在下一次压缩生效。超时、失败和取消时的历史保留规则保持原有回归约束。

## 补全的审查问题

| 原 PR 的问题 | 集成修正 |
| --- | --- |
| 数据库中的合法 JSON 可带入 0 或超大时限 | 读写均执行 30–3600 秒范围约束 |
| 演示 API 没有新接口，卡片一直加载 | 补齐演示 GET/PUT 和默认值 |
| 保存期间可以编辑或再次按 Enter | 输入、保存、取消在请求期间禁用；保存入口与 Enter 处理均检查状态 |
| 非回环监听会暴露未认证的管理 API | 根据实际绑定的监听器地址组装路由；非回环时管理 API、控制台和桌面扩展路由不挂载，管理命名空间返回 403，避免落入 Cursor 上游代理 |

管理边界由 `server/src/app.rs` 持有，桌面壳通过 `merge_control_router` 添加同一边界内的原生能力和静态资源。默认桌面监听为回环地址；独立服务若绑定 `0.0.0.0`、`::` 或 LAN 地址，管理界面不再可用。`/__byok-api__/healthz` 健康探针和既有模型协议路由保留。回环服务的端口转发/反向代理由部署者控制；本改动没有新增远程管理身份认证。

## 本地验证

- `cargo test --locked --workspace --all-targets`：629 通过、0 失败、14 忽略。忽略项包括需要外部环境的测试，不能视为已验收。
- `cargo clippy --locked --workspace --all-targets -- -D warnings` 通过；格式和差异检查通过。
- 存储测试覆盖默认 180、保存及读取范围约束；控制 API 测试覆盖读写回读、越界归一和负数/小数/字符串/null 拒绝，拒绝后原值保留。
- 慢摘要用例先以 30 秒配置启动同一服务，再保存 600；摘要等待 31 秒后返回，摘要落库且对话正常续答，无需重启。
- 两个停滞用例显式设置 30 秒，验证小历史和大历史超时后完整保留。第一次全量检查发现大历史用例仍依赖旧默认值，修改其配置后最终全量通过。
- 管理路由测试覆盖 IPv4/IPv6 回环、通配地址与 LAN 地址，检查 GET/PUT/POST/DELETE，以及桌面资源/能力路径，确认不会转发到上游。
- `npm run check`、`npm run i18n:scan`、`npm run build:demo` 通过，四种语言无缺失。
- Chromium 演示 UI：默认 180 可见、29 被拒绝、600/900 保存回显；挂起 PUT 时输入框、保存、取消均禁用，每次提交仅一条 PUT。
- 800×600 浏览器视口下设置导航与卡片可见，无文档水平溢出。
- macOS Tauri debug/no-bundle 构建通过。初次编译遇到本机 SQLx 宏动态库的 `mis-aligned LINKEDIT string pool`，沿用既有本地参数 `--config 'profile.dev.package.sqlx-macros.strip="none"'` 重建后成功，未修改发布构建配置。上述 Rust 测试和 Clippy 也使用同一参数。

演示 Vite 记录了 `ResizeObserver loop completed with undelivered notifications` 警告；上述设置交互与小视口检查通过，没有据此声明整页 UI 无警告。

以上为本地自动化及演示 UI 验证，不等同于真实 Cursor 长会话或真实慢供应商验收。本次没有运行 20 分钟任务。

## 合并验证

集成 head `1a2d71d5b6cf12d32d653225cb4186d0bfcf9ebd` 的 [分支 CI](https://github.com/haohaomin/cursor-byok/actions/runs/45942964744) 和 [PR CI](https://github.com/haohaomin/cursor-byok/actions/runs/45943026147) 均通过 Linux、macOS、Windows 与前端检查。Codex 云端审查在同一 head 完成，无新发现。Sourcery 因最近七天审查额度用尽跳过；不能把 skipped 记作复审通过。原 #56 的四项意见已在上表逐项补全。

HTTPS Git 传输本轮出现连接停滞；核对 GitHub 官方 API 提供的主机公钥后，使用现有 SSH 身份通过 443 端口完成推送。未修改全局网络配置。

## 发布核验

合并后的 [main CI](https://github.com/haohaomin/cursor-byok/actions/runs/45944074385) 全部通过；[自动发布](https://github.com/haohaomin/cursor-byok/actions/runs/45944843713) 四平台成功。v1.2.9 的 20 项资产哈希、7 份签名与 3 份更新清单全部核验通过，发布提交 `c4c5e76af7f2e6d0da117804c8453fc00f82a754`，五处版本均为 1.2.9。见 [发布记录](../releases.md#v129) 和 [结构化证据](../2026-11-27-v1.2.9-release-evidence.json)。未安装到用户机器。
