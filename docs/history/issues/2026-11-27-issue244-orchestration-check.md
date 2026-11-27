# Issue #244 / PR #245：旧编排补丁与当前客户端对照

核对日期：2026-11-27。当前基线：`f38b579682e17d66e245099ac80d7bb6e964d576`，公开版本 v1.2.13。来源 [Issue #244](https://github.com/leookun/cursor-byok/issues/244) 与关联 [PR #245](https://github.com/leookun/cursor-byok/pull/245) 均为 OPEN；不是本 fork 的待合并 PR。

## 结论

**不采用 PR #245 原补丁，不恢复旧工具别名或旧 Projects 模式编号 6。** 该补丁基于 Go 后端和 Cursor 3.14.7，存在创建命令未发送、提示要求调用缺失工具等实际断点。当前 Cursor 3.31.15 已移除该 AgentMode 枚举值，不能只增加模式映射就称为支持当前 Projects。

当前 `Task` 已携带新建/恢复子代理的任务内容、后台选项和父会话关系；这些现有能力不等同于完整项目编排。本轮只做审查、隔离探针和既有回归，**没有生产代码修改、没有新发版，也没有关闭 Issue**。

## 原需求逐项状态

| #244 的要求 | 当前证据 | 结论与剩余工作 |
| --- | --- | --- |
| create-agent / send-message-to-agent / AWAIT | 当前 Task 的新建和 resume 均发 SubagentArgs，prompt、model、run_in_background、interrupt 和父/根会话在 protobuf 往返后保持；旧 create-agent 则只发 ForceBackgroundSubagentArgs | 新建/恢复的部分需求已有覆盖；独立消息投递、fork 和现代 Await 不能由此推断完成。旧别名不移植 |
| AGENT_MODE_PROJECT=6 与 Projects | 隔离请求 mode=6 得到 invalid_argument，模型调用为 0；当前客户端本地 runtime 与 Agent Host 的 AgentMode 定义均无 6/PROJECT | 旧模式确实仍被拒绝，但它已不是当前客户端的模式契约。现代 Projects 需要协调器通信、Agent Store 与实际入口调查，保持未完成 |
| 全模式 prompt 与工具同步 | 旧 Projects prompt 要求用户通信只能走 send_message，但工具目录没有该工具；当前本地 runtime 项目提示使用协调器和 SendToAgent 等不同契约 | 不整体覆盖当前提示。当前项目模式所需工具及可见输出必须一起实现、验证，不能只复制提示 |
| AgentStoreConflict、MountedAgentStore、parent_spawn_* 等 Proto | 当前 agent_v1 已有 conflict Exec oneof 54、RequestContext.mounted_agent_stores=25；当前 aiserver_v1.CloudSubagentParentReference 已含 parent_spawn_kind/id 并被其他消息引用 | 原来的缺声明判断已过时；声明/接线存在不证明 Agent Store 同步、冲突处理或云端父子生命周期已经可用 |
| 六个 feature gates | long_running_jobs 已固定开启；旧项目及 Agent Store 开关不会自动补齐执行器、同步和用户通信 | 不批量开启。现代项目路径和 UI/服务端支持仍需逐项验证 |

当前客户端仍包含按能力条件构造的 `Await` 工具及 SubagentAwait 执行器，不能因旧工具名不再出现就把等待需求一并认定过时。本轮没有新增 Await，也未验证实际窗口是否启用该工具。

## PR #245 的固定版本审查

固定 head：`a29f7190b2cdc724515cfe55c3ee481b41cc321c`；26 文件，+2872/-212，非 draft。已读取正文、完整 diff、固定 head 文件、评论、reviews 和检查；评论/reviews/head checks 均为空。文件及 patch 哈希见 [结构化证据](2026-11-27-issue244-evidence.json)。

1. **创建代理没有创建请求。** [bridge.go:847](https://github.com/leookun/cursor-byok/blob/a29f7190b2cdc724515cfe55c3ee481b41cc321c/internal/backend/agent/bridge/exec/bridge.go#L847) 的 openCreateAgent 只检查 JSON 后发送 `ForceBackgroundSubagentArgs{tool_call_id}`，不读取 prompt、fork、attachments 或后台选项。[events.go:380](https://github.com/leookun/cursor-byok/blob/a29f7190b2cdc724515cfe55c3ee481b41cc321c/internal/backend/forwarder/events.go#L380) 只补 Task 展示卡片，未发送 SubagentArgs。完成结果只有 accepted/not found 等文字，没有供后续调用使用的 agent ID。
2. **Projects 提示要求调用缺失工具。** [prompt/projects/prompt.md:28](https://github.com/leookun/cursor-byok/blob/a29f7190b2cdc724515cfe55c3ee481b41cc321c/prompt/projects/prompt.md#L28) 要求所有用户通信走 send_message；agent、multitask、projects、orchestrator 四份目录相同，24 项工具均无它，执行桥也没有对应实现。因此新增提示并未形成可用的用户输出路径。
3. **新类型未接到旧协议入口。** 该 head 的 AgentStoreConflict、MountedAgentStore 和 CloudSubagentParentReference 仅增加声明，未接入 Exec oneof 或 RequestContext。当前 Rust 协议已有相关接线，不能再次复制旧声明，也不能把类型存在当作运行能力完成。
4. **发送消息只是普通恢复调用。** [bridge.go:878](https://github.com/leookun/cursor-byok/blob/a29f7190b2cdc724515cfe55c3ee481b41cc321c/internal/backend/agent/bridge/exec/bridge.go#L878) 设置 `resume_agent_id` 和 prompt，却不处理 `responding_to_message_ids`，也不设置 interrupt/background。这不是独立消息投递或 fork 的实现。
5. **AWAIT 没有作者提供的成功闭环。** [bridge.go:941](https://github.com/leookun/cursor-byok/blob/a29f7190b2cdc724515cfe55c3ee481b41cc321c/internal/backend/agent/bridge/exec/bridge.go#L941) 有等待请求及四类返回映射，但 PR 正文明确记载所测 CLI 缺少对应 handler。正文的 Task 成功不能用来证明新增 create-agent/AWAIT 成功。

依赖 [PR #241](https://github.com/leookun/cursor-byok/pull/241) 已于 2026-08-03 合并，最终 head 为 `5df3e8f5120fc08842fa506be1f8c894e6d72825`；#245 仍带其旧版本提交 `2c650b37659880c8f18f551bee946a16d76d820d` 的五文件端点改动，不能整体重复采用。新增 orchestrator 提示仅为资产，PR 自己也承认没有独立协议模式。

## 当前路径与客户端证据

```text
server/
├── prompt/cursor/modes/                 # 当前根代理/子代理工具目录，无旧编排别名
├── src/cursor/tools/codec/request.rs    # Task → SubagentArgs，携带任务及恢复参数
├── src/cursor/tools/tool_call_result/   # 客户端结果 → 模型结果与原生卡片
├── src/cursor/conversation/             # 父轮运行、完成通知和用户插入的生命周期
├── src/cursor/checkpoint/               # 会话持久化与后续轮次恢复
└── tests/                              # 工具回合、子代理准入和中断回归
protocols/cursor/                       # 当前 protobuf 类型和消息字段接线
```

```text
当前 Task 新建 / resume
  → BYOK 发送 SubagentArgs（prompt、model、resume、background、父子关系）
  → Cursor 执行器负责子代理运行
  → SubagentResult → BYOK 保存工具结果 / checkpoint → 模型继续
  → 后台任务的后续通知可启动父会话新 Run（不是同一 Run 内永远等待）

旧 PR 的 create-agent
  → 仅 Task 展示 + ForceBackgroundSubagentArgs（新工具调用 ID）
  → 当前客户端查找同 ID 的已有执行，或预记将来的转后台意图
  → 可以返回 ACCEPTED，但这条路径没有创建代理
```

只读核对 Cursor **3.31.15** 三个已安装文件，SHA-256 与上一轮身份记录一致。本地 runtime 和 Agent Host 的 AgentMode 只有 0、1、2、3、4、5、7、8，没有旧 6/PROJECT；三文件均未匹配到旧 create-agent/send-message-to-agent 字面量。这个负向搜索只限这些文件，不是整个产品没有其他编排功能的证明。

Workbench 的 `forceBackgroundByToolCallId`（字符区间 16464027–16464468）查找 activeForegroundExecutions；找不到时仅向 pendingForegroundBackgroundToolCallIds 加入 ID 并返回 ACCEPTED。提取这一方法，用隔离 VM、假的 Map/枚举/结果类执行：

| 输入状态 | 结果 |
| --- | --- |
| 没有任何已有执行，传新工具 ID | ACCEPTED；活动执行仍为 0，只新增一个待转后台 ID |
| 已有对应执行 | 调用 requestBackground 一次，活动执行数不增加 |

这验证了方法语义，**不是启动真实 Cursor 子代理或 UI 的验收**。没有执行应用 bootstrap、网络调用或读取用户对话。

REA 身份记录复用 `ev_a485fa596bb60a5bf97cc88b22f0197ca1c4331db3c49acfa96d3cc6ff31806b`，其分析覆盖为 partial，Agent Host 当时存在 1024 MiB worker 限制，workbench 仅做本次定向文本阅读。没有重复宣称完整调用图或新的 REA 全包分析。

## 验证与复跑

- 临时诊断 **2 项通过**：Task 新建/resume 的 protobuf 往返；旧 mode=6 的生产传输层错误及零模型调用。第二项“通过”指确认拒绝现状，不是 Projects 修复成功。
- 当前基线相关回归 **59 通过、0 失败**：interrupt 24、subagent_admission 13、tool_round 22，含后台结果、中断、父任务后续恢复和工具结果呈现。
- 客户端提取方法的两种状态对照通过；脚本绑定具体文件哈希，版本变化时会停止而非沿用旧偏移。
- 本轮未跑工作区全量，不沿用上一轮 664 项作为新行为的实机证据；无新生产代码需要重新发布。

保留 [Rust 探针](reproductions/issue244_current_protocol.rs) 和 [客户端契约探针](reproductions/issue244_client_contract.py)。Rust 探针临时复制到 `server/tests/issue244_probe.rs` 后执行，完成后已移除临时测试入口；原文件保存在历史目录，不将“永久拒绝模式 6”固化为产品测试要求。

```sh
CARGO_TARGET_DIR=/Users/renhao/works/cursor-byok/target cargo test --config 'profile.dev.package.sqlx-macros.strip="none"' --locked -p cursor-server --test issue244_probe -- --nocapture
CARGO_TARGET_DIR=/Users/renhao/works/cursor-byok/target cargo test --config 'profile.dev.package.sqlx-macros.strip="none"' --locked -p cursor-server --test tool_round --test subagent_admission --test interrupt
python3 docs/history/issues/reproductions/issue244_client_contract.py /Applications/Cursor.app/Contents/Resources/app
```

SQLx 参数仅处理本机构建工具链，没有修改工程配置。全部输入为隔离数据；没有修改 Cursor 安装文件、正式代理或数据库，没有测试真实供应商。原问题保持未完成，后续应基于当前客户端实现现代 Await、项目通信和 Agent Store 所需的真实执行路径，而不是恢复此旧补丁。
