# 工作状态

更新：2026-11-27；v1.2.12 已发布并完成下载、签名和更新清单核验。历史测试与环境状态保留原核对日期。逐项调查见 [Issue 清单](issues.md)，原始测试见 [历史资料](history/README.md)。

**2026-11-27 Issue #327：**目标模式已在隔离协议测试中实现创建、续跑、完成、暂停/恢复和清除；补齐并发、延迟通知与压缩恢复保护。12 项目标测试、子代理续跑测试、663 项工作区回归及 Clippy 通过，正在提交审查；真实 Cursor 自动通知/原生 UI 与供应商尚未验收。[验证记录](history/issues/2026-11-27-issue327-goal-mode.md)。

**2026-11-27 Issue #251：**角色正文漏传修复已由 [PR #64](https://github.com/haohaomin/cursor-byok/pull/64) 合入 main，已随 v1.2.12 发布并完成全部资产、签名和更新清单核验。审查补齐清空正文、删除定义和取消选择时的明确停用状态；五项定向测试、650 项工作区回归、Clippy 及跨平台 CI 通过。真实 Cursor/模型未验收，提示词编辑面板仍待评估。[验证记录](history/issues/2026-11-27-issue251-custom-subagent-role.md)。

**2026-11-13 进展：**#150/#417 的逐模型 Token 定价已通过 [PR #39](https://github.com/haohaomin/cursor-byok/pull/39) 合入 `main`（合并提交 `e53e97e`）；原 #35 是合入已先落地的 #34 分支，未直接进入主线，已由 #39 在当前主线基线上重新集成。设置页按 `model_hash` 保存输入、输出、缓存读取和缓存写入单价；首页总计与按模型明细按覆盖价计算，未配置覆盖时使用全局价格。当前主线联合回归为 293 个服务端单测通过、0 失败、14 忽略，桌面检查、Clippy 和生产构建通过。插件模型暂使用全局价格，真实供应商和发布版本仍待验收，见 [定价验证记录](history/issues/2026-11-13-issue150-model-pricing.md)。

**当前结论：2026-11-27 已将 PR #13、#43–#49、#52、#53、#57、#59、#60、#62、#63 和 #64 合入 main。最新公开版本为 v1.2.12，包含此前合并、PR #50 的 HTTP/2、PR #51 的自动压缩、PR #52 的 Todo、PR #53 的团队配置、PR #57 的 DeepSeek 跨协议 reasoning 及 PR #59 的二进制 BidiAppend 修复，以及 PR #60 的自动压缩时限设置、PR #62 的摘要输出上限与有限重试、PR #63 的 HTTP 请求体和解压大小限制，以及 PR #64 的自定义子代理角色传递与明确停用。真实 OAuth 账号、Semble 批准/拒绝、DeepSeek 账号和 Cursor 原生 Agent UI 仍待验收；Windows Cursor UI 的 CA 初始化与接管已通过。**

**2026-11-13 进展：**上游 PR #433 已按当前 Rust/Tauri 架构选择性重实现：非 Tab 的 Cursor host 请求进入本地路由，CONNECT 隧道保持原始 authority；硬编码云端 400/404 短路和 `custom-protocol` feature 未采用。定向代理测试、Clippy、桌面检查和工作区回归通过，真实代理网络与 Cursor UI 待验，见[验证记录](history/pull-requests/2026-11-13-pr433-proxy-routing.md)。**

**2026-11-13 进展：Issue #505 的多个 reasoning signature 修复已通过 fork PR #12 合入 `main`（合并提交 `21f0b3c`），已随 v1.2.1 发布；`cursor-server --all-targets` 通过（465 通过、14 忽略）。真实 Cursor 与原网关仍未验收，不能据此宣称所有供应商都已覆盖。**

**2026-11-13 进展：Issue #463/#184 已增加 Google Gemini OpenAI 兼容预设，使用完整 `v1beta/openai/chat/completions` 地址和现有 Bearer 鉴权；前端类型检查、构建与浏览器配置验证通过，已通过 [PR #36](https://github.com/haohaomin/cursor-byok/pull/36) 合入 `main`。原生 `generateContent` 协议和真实 Gemini Key 尚未验收，见 [验证记录](history/issues/2026-11-13-issue463-gemini-openai-compat.md)。**

**2026-11-24 Issue #102：**工具 ID 混用在当前历史回放链路未复现。补充 Chat → Responses 和 Responses 自身回放两项自动化，相关回归共 36 项通过；未改生产代码，原作者旧历史与真实客户端仍待验，见 [复测记录](history/issues/2026-11-24-issue102-check.md)。

**2026-11-23 Windows 验证：**有效 CA 的普通用户安装/检测/移除闭环已通过，补齐了[初测](history/issues/2026-11-21-windows-ca-check.md)使用过期证书的边界。完整生产模块五种状态对照通过；桌面手动安装后为 Ready，Windows 默认 TLS 客户端的 HTTPS 成功、错误主机名被拒绝，移除后恢复 Untrusted 并拒绝连接。v1.2.1 安装包的 CA 初始化和 Cursor 接管 UI 也已通过；真实 Agent 验收尚未完成：提交与 provider 存活未可靠确认，账号 401 和空日志不足以定位根因（2026-11-24 更正）。临时私钥、目录、任务及快捷方式已清理，见 [CA 记录](history/issues/2026-11-23-windows-ca-valid-check.md) 和 [Agent 记录](history/issues/2026-11-23-windows-cursor-agent-acceptance.md)。

**2026-11-24 Issue #444：**自动压缩丢弃早期轮次及失败兜底丢失历史已有失败复现；分批摘要、失败保留和中断处理已通过 [PR #51](https://github.com/haohaomin/cursor-byok/pull/51)；合入 main；审查补充后本机 597 项回归、Clippy 和最终跨平台 CI 通过，已随 v1.2.4 发布并完成下载/签名核验。真实 Grok 未验收，见 [记录](history/issues/2026-11-24-issue444-compaction-retention.md)。

**2026-11-24 PR #493：**选择性修复经 fork PR #50 合入，跨平台 CI 与 v1.2.3 发布核验通过。修复 Cursor 代理 HTTP/2 协商和响应取消后上传未释放；其他网关保持 HTTP/1.1。[测试与范围](history/pull-requests/2026-11-24-pr493-http2-relay.md)。

**2026-11-24 PR #485 Todo：**完成回包只保留 patch、压缩后无客户端检查点丢失 Todo 已修复；[PR #52](https://github.com/haohaomin/cursor-byok/pull/52) 已合入 main，548 项服务端回归和最终跨平台 CI 通过。已随 v1.2.5 发布核验，原生 Cursor 卡片 UI 未验收，见 [记录](history/pull-requests/2026-11-24-pr485-todo-state.md)。

**2026-11-24 PR #485 团队配置：**两个无团队配置接口的本地路由已复现并修复，[PR #53](https://github.com/haohaomin/cursor-byok/pull/53) 合入 main。309 项单元测试、Clippy 和最终跨平台 CI 通过，已随 v1.2.6 发布核验；未扩大 Windows Agent 与真实团队账号验收结论，见 [记录](history/pull-requests/2026-11-24-pr485-team-metadata.md)。

**2026-11-26 PR #485 DeepSeek 初审：**跨 Responses/Chat 续写缺少原生明文 reasoning、最终快照 reasoning 丢失，以及 Chat 显式空 reasoning 被过滤的问题已在隔离 HTTP 服务中复现并独立修复。修复只跨协议转发完整明文 `reasoning_text`，保留原生 Chat 状态和显式空值；摘要、加密或不完整内容不会伪造。该修复已由 fork [PR #57](https://github.com/haohaomin/cursor-byok/pull/57) 合入并随 v1.2.7 发布；未做真实 DeepSeek 账号或 Cursor 原生 Agent UI 验收，见 [记录](history/pull-requests/2026-11-24-pr485-deepseek-review.md)。

**2026-11-26 PR #485 DeepSeek 发布：**fork [PR #57](https://github.com/haohaomin/cursor-byok/pull/57) 已合入，Pull Request CI 的 Linux、Windows、macOS、前端和 Sourcery 检查全部通过；v1.2.7 已完成 20 项资产、7 个签名和三个更新清单核验。真实 DeepSeek 账号和 Cursor 原生 Agent UI 仍待验收，见 [记录](history/pull-requests/2026-11-24-pr485-deepseek-review.md) 与 [发布核验](history/releases.md#v127)。

**2026-11-26 本 fork PR #55：**二进制 BidiAppend 被拒绝已复现并修复，经 [PR #59](https://github.com/haohaomin/cursor-byok/pull/59) 合入；原 #55 已标记合并，Issue #54 已关闭。560 项服务端回归、Clippy、跨平台 CI 及 v1.2.8 发布核验通过。真实 Cursor UI/供应商待复测；压缩超时设置由 #56 单独评估，共用 HTTP 入口的大请求上限是独立待办，见 [验证记录](history/pull-requests/2026-11-26-pr55-binary-append.md)。

**2026-11-27 本 fork PR #56：**已在 `codex/pr56-compaction-settings` 采用并补齐自动压缩时限设置，本地工作区 629 项、Clippy、前端构建及演示 UI 保存验证通过。默认 180 秒，支持 30–3600 秒；下一次自动压缩生效。已通过 PR #60 合入 main，原 #56 标记合并；跨平台 CI 与 Codex 云端审查通过，v1.2.9 已发布核验，见 [验证记录](history/pull-requests/2026-11-27-pr56-compaction-timeout.md)。

**2026-11-27 本 fork PR #61：**已通过 PR #62 合入 main，原 #61 标记合并；摘要输出设置、模型/上下文预算及每批有限重试已补齐。635 项回归、最终跨平台 CI 通过，v1.2.10 已发布并完成 20 项资产、7 份签名和三个更新清单核验。见 [验证记录](history/pull-requests/2026-11-27-pr61-compaction-output-cap.md)。

**2026-11-27 HTTP 大请求限制：**经 PR #63 合入并随 v1.2.11 发布；压缩前后各限 64 MiB，健康检查立即返回，Tab/文件上传保持流式传输。最终 645 项回归、Clippy、跨平台 CI、20 项资产/7 份签名/三个更新清单核验通过，真实 Cursor 超限 UI 待验；[验证记录](history/issues/2026-11-27-cursor-request-body-limit.md)。

## 1. 已发布的成果

v1.2.11 新增 HTTP 请求体与解压大小限制（[发布核验](history/releases.md#v1211)）；v1.2.10 新增摘要输出上限及每批有限重试（[发布核验](history/releases.md#v1210)）；v1.2.9 新增自动压缩时限设置及本地管理接口边界（[发布核验](history/releases.md#v129)）；v1.2.8 新增二进制 BidiAppend 修复（[发布核验](history/releases.md#v128)）；v1.2.7 新增 DeepSeek 跨协议 reasoning 回放修复（[发布核验](history/releases.md#v127)）；v1.2.6 新增本地身份团队配置路由（[发布核验](history/releases.md#v126)）；v1.2.5 新增 Todo 完整列表与压缩恢复（[发布核验](history/releases.md#v125)）；v1.2.4 新增自动压缩修复（[发布核验](history/releases.md#v124)）；v1.2.3 新增 HTTP/2 代理修复（[发布核验](history/releases.md#v123)）；v1.2.1 已涵盖 v1.1.0 之后的模型备份、批量添加、逐模型定价、Windows 用户级 CA、上下文和终端修复，见 [发布与资产核验](history/releases.md#v121)。发布没有扩大真实账号、客户端审批等未完成场景的验收范围。

| 工作 | 已完成部分 | 保留的边界 |
| --- | --- | --- |
| Responses 文字缺失、子代理启动及结果交付 | #410、#477、#340 的具体缺口已修复，分别有自动化或真实 Cursor 验证，进入已有版本 | 不代表所有供应商、Windows 原环境和后台 Shell 边界都覆盖 |
| 子代理模型选择与中断历史 | #449 模型名单/选择已获用户验收；#172 中断后的文字与已完成工具保留修复已合入已有版本 | #172 相关实机边界继续以原记录为准 |
| #155 后台命令结束后不续答 | v1.0.17 已包含服务端修复；普通 25 秒后台任务自动续答及完成通知去重重放已通过 | 按用户要求仍为“完成一半”；真实审批等待、压缩期间完成等未完整验收；不做 20 分钟测试 |

## 2. 已合入 main 的近期工作

2026-11-21 通过 GitHub API 与 Git 核对：远端 `main` 为 `68c4dfbc99bcba8f5e56d6e0535b4f2c92a584de`。本 fork 没有开放 PR。源码合入状态不能替代安装包发布状态。

| 工作 | main 中的结果 | 证据 |
| --- | --- | --- |
| #511 SQLite 清理后文件不缩小 | 临时库复现后修复空间回收、并发及失败恢复；PR #1 已合入 | [修复与测试](history/issues/2026-10-27-issue511-storage-fix.md)；原作者大库的增长来源尚未定位 |
| #504 重复用户消息中断工具 | 同一消息 ID 在排队前去重，新 ID 的相同文字仍作为新输入；PR #2 已合入 | [修复与边界](history/issues/2026-10-27-issue504-input-redelivery-fix.md)；不声称原报告全部症状都解决 |
| #337 排队消息、子代理思考参数 | 补充协议与实际 HTTP 请求回归；PR #3 已合入 | [调查记录](history/issues/2026-10-27-issue337-check.md)；仅测试与文档，受控场景未复现，无新生产修复 |
| #373 压缩失败后继续对话 | 补充压缩失败、重试、恢复和结束帧判断测试；PR #5 已合入 | [调查记录](history/issues/2026-10-27-issue373-check.md)；仅测试与文档，原 UI 待复测 |
| 上游 PR #515 子代理消息身份方案 | 保留候选实验和三项失败证据，未采用原补丁；本 fork PR #7 已合入记录 | [评审记录](history/pull-requests/2026-10-28-pr515-check.md)；不是合入上游 #515 的代码 |

其他此前已进入源码的修复（系统代理缓存、SQLite 事务恢复、缓存键、MCP schema 等）继续见 [维护状态](maintenance-status.md)，本轮不改写它们的验收结论。

## 3. 2026-11-04 合入的实现

按维护者要求完成源码合并。#8 先合入；#4 同步新 main、解决文档迁移冲突后合入；#6 同步 #4 并改为以 main 为目标，随后合入。生产源码没有产生需要手工选择的合并冲突。以下实现已随 v1.1.0 发布。

| PR | 合并提交 | 已合入内容 | 仍待验证 |
| --- | --- | --- | --- |
| [PR #8](https://github.com/haohaomin/cursor-byok/pull/8) | `07f9951` | 远程采集、忽略规则、分页、Python 导入隔离 | 独立终端审批卡片补丁已通过 #13 合入并随 v1.2.1 发布；真实批准/拒绝和 Windows Cursor UI |
| [PR #4](https://github.com/haohaomin/cursor-byok/pull/4) | `e24eddf` | Grok 自动/手动续期、凭据轮换、插件 API 2、详细日志认证重试 | 真实 Grok 账号；API 1 外部插件需适配 API 2 |
| [PR #6](https://github.com/haohaomin/cursor-byok/pull/6) | `91db826` | GPT/Google 续期、有限重试、冷却、旧凭据保护、Google 手动配额刷新 | 真实 GPT/Google 账号 |

本轮合并记录与 CI 见 [集成记录](history/2026-11-04-branch-integration.md)。OAuth 详细记录已进入当前文档目录：[Grok](history/issues/2026-10-27-issue445-check.md)、[GPT/Google](history/issues/2026-10-28-issue481-check.md)。这些源码已进入 v1.1.0 安装包；合入、构建及自动化通过不等于真实账号或客户端验收。

## 4. 审批修复已合入，真实 UI 待验收

[PR #13](https://github.com/haohaomin/cursor-byok/pull/13) 已于 2026-11-19 合入，提交 `d3fdc07`，已随 v1.2.1 发布。下述 2026-10/11 调查与测试保留原日期，未提交状态已被这次合并取代；真实批准/拒绝仍未验收。

真实 Cursor **3.27.22** 的 SSH 测试已发出一次 SembleSearch。BYOK 收到的唯一 Exec 事件是 `rejected: Review cancelled or failed`，没有 Start/Exit。用户没有点击拒绝，因此这次不能算“拒绝验收通过”。请求与两次模型调用均正常结束，见 [脱敏证据](history/issues/2026-10-30-issue469-approval-evidence.json)。

代码检查发现：Semble 显示为 MCP 卡片，内部采集 Shell 却复用其调用 ID；安装客户端的终端审查路径只为终端卡片创建 review model。新增回归在基线确实失败，候选修复后通过。静态分析支持这一原因，但修复后真实 Cursor 是否完整展示审批仍须实测。

```text
server/src/cursor/
├── tools/semble/collection.rs       # 新增：每次采集的独立终端卡片与执行请求
├── tools/semble.rs                  # 采集阶段推进，只生成一次父搜索结果
├── tools/runtime.rs                 # 执行所有权；取消时终止采集并关闭卡片
├── tools/codec/response.rs          # 多条客户端呈现事件与断流处理
├── tools/tool_call_dispatch/search.rs # 启动时先发终端卡片，再发 Exec
├── tools/mod.rs                     # 传递取消产生的呈现事件
├── conversation/runtime.rs          # 向 Cursor 发送执行和卡片事件
├── conversation/output.rs           # 输出与中断路径的收尾
└── tools/semble/tests.rs            # 审批身份、拒绝、取消、重复事件回归
server/tests/tool_round.rs            # 断流错误结果的回归适配
```

这部分包含 **8 个生产代码文件、2 个测试文件**，已通过 #13 进入 main 和 v1.2.1；不再是本地未提交候选。终端卡片先于 Shell 请求，ID 与 MCP 卡片不同；采集分页、完成、拒绝、取消和断流都有收尾；原始源码采集正文不额外进入模型工具结果。没有设置跳过审批，没有修改 Cursor 安装文件。

## 5. 已有测试结果

以下结果核对于 2026-11-01，取自当时保留的实际执行日志。2026-11-02 仅整理文档，没有重新运行程序测试。

| 检查 | 实际结果 | 能证明的范围 |
| --- | --- | --- |
| 新增审批身份回归，修复前 | 1 项失败，原因是 Shell 与 MCP 调用 ID 相同 | 已有代码缺口，不是凭推测加改动 |
| `cargo test --workspace` | **522 通过、0 失败、1 忽略** | 当前候选的工作区自动化 |
| 单独执行忽略的真实嵌入模型测试 | **1 通过**；普通及相关搜索两阶段采集、过滤均通过 | 真正运行主机命令和缓存嵌入模型，未经过 Cursor 审批 UI |
| `cargo clippy --workspace --all-targets -- -D warnings` | 通过 | Rust 静态检查 |
| `cargo fmt --all -- --check`、`git diff --check` | 2026-11-01 重新执行，通过 | 当时工作区的格式、差异有效性 |
| 修复后 Cursor 点击批准 / 拒绝 | **未完成** | 后续界面工具报 ScreenCaptureKit `-3812`；不能用自动化代替 |

本节的 522 项结果属于当时 remote-semble 的本地审批候选，不能移作 OAuth 分支的测试证据。2026-11-04 合并版本的独立 CI 计数与日志见 [集成记录](history/2026-11-04-branch-integration.md)，这些历史 CI 不覆盖当时尚未合入的审批补丁。

## 6. 环境收尾与剩余事项

以下环境状态核对于 2026-11-01；2026-11-02 文档整理未再次操作 Cursor 或远端测试环境。

- 本机 Cursor 设置已按原始字节恢复，当时重新核对 SHA-256 与原备份记录一致。
- 远端临时 `cursorAgentHost.remoteInferenceRoute=always` 已撤销，Machine 设置恢复为原本不存在；当时 SSH 只读核对通过。
- 隔离服务已经停止，50745/50746 两个端口关闭；正式后端健康检查为 204。
- 已清理测试期间含凭据的临时数据库副本、设置备份、服务日志和重复 example；脱敏证据、测试日志及合成远端工作区保留用于复核。
- 测试时临时移除的 Command Allowlist `python3` 项：**尚无恢复确认**。它不保存在上述 settings.json 中，不能用设置文件摘要宣称已恢复。
- 审批补丁已通过 #13 合入 main，并包含在 v1.2.1 安装包中。

后续顺序：确认允许列表恢复情况 → 在可操作的 Cursor 中验收已合入的审批修复（批准后只执行一次、拒绝不执行、等待不超时）。v1.2.1 发布及资产核验已完成。OAuth 实现已合入，真实账号验收等待可用测试账号；#155 的未覆盖边界独立保留，不以 #469 的搜索测试代替。
