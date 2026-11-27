# Issue #244：当前 Cursor 的子代理通知接收

日期：2026-11-27。基线 `11ccadb21a7d1d53a4e57f8aee61e6315051cb22`，公开版本 v1.2.13。本项是 [#244 / PR #245 审查](2026-11-27-issue244-orchestration-check.md) 后发现的独立协议缺口；不采用旧 Go 补丁，也不把完整项目编排标为完成。

## 已复现的故障

Cursor 3.31.15 的本地 runtime 已声明并消费四种通知：`WORKER_REPARENTED=3`、`WORKER_MESSAGE=4`、`WORKER_NEEDS_ATTENTION=5`、`PROJECT_SESSION_MESSAGE=6`。原服务端只认识完成和进度，因此收到新通知就以 `invalid_argument / unknown background task completion reason` 结束，内容无法送到父对话。

当前协议还包含字段 12 `completed_at_ms`、13 `record_only`。原声明缺少它们：同一任务内多条消息缺少时间身份；带 `record_only=true` 的完成通知仍调用模型。三个基线测试全部失败，其中 record-only 应只回一个结束事件，实际发出 14 个事件并调用模型。

为消息增加身份后，另一个测试证明：按事件哈希排序会倒置同批消息。已改为保留客户端数组顺序；事件身份仅用于去重。

以上是当前安装包的静态契约与合成 protobuf 请求的复现，**不是当前 Cursor 窗口自动发送这些通知的实测**。安装包、源码偏移、哈希与失败日志摘要见 [结构化证据](2026-11-27-issue244-notifications-evidence.json)。

## 修复后的行为

| 输入 | 模型收到的含义 |
| --- | --- |
| WorkerMessage | 子代理发来消息，仍可能运行；不冒称任务完成 |
| WorkerNeedsAttention | 等待用户回答或认证；提示用户处理，不代为批准、回答或轮询 |
| WorkerReparented | 子代理被转交，记录协调关系；不冒称任务完成 |
| ProjectSessionMessage | 其他项目聊天的代理消息，按不可信信息处理；允许缺少子代理/工具调用 ID |
| 用户主动操作的子代理更新 | 保留 `started_by: user`，提示不要接管用户正在操作的线程 |
| record_only | 成功确认、不唤醒模型，也不消耗以后真正投递的事件身份 |
| TaskProgress | 保持原有不唤醒策略 |
| TaskFinished | 保持完成通知与已有去重身份 |

新消息身份包含任务、可选子代理/工具调用 ID、原因、可选时间和正文摘要；同任务、同正文但不同时间的消息不会互相吞掉。完全相同事件在重试、拆分/重组批次、服务协调器重建后不会重复调用模型。数据库仍使用既有消息记录，不新增表或迁移。

```text
Cursor BackgroundTaskCompletionAction
  → compile/insert_messages.rs       # 区分通知语义，按输入顺序产生稳定事件
  → conversation/background.rs       # 运行中占位 + 已持久化消息身份去重
  → conversation/runtime.rs          # 父 Run 活跃时追加；空闲时创建后续 Run
  → store / checkpoint               # 持久化消息与可恢复历史
  → provider                         # 旧历史保持完整前缀，仅追加新通知
```

本补丁修改两个 Proto 声明及 `compile/insert_messages.rs`（303 行），测试集中在新增 `conversation_delivery/worker_notifications.rs`（343 行）并由原套件加载；以上行数含空行、注释，生产文件与测试分开统计。

## 验证与边界

七项定向回归覆盖四类通知、完成后的独立投递、重复正文/时间、重试/重建去重、record-only、混合重组批次、客户端消息顺序、用户所有权与父 Run 活跃时投递。测试比较模型请求的结构前缀与提示稳定性，而非仅检查是否返回成功。

最终通知整套 **25 通过**；工作区 **671 通过、0 失败、14 忽略**；Clippy `--workspace --all-targets -- -D warnings`、格式和 diff 检查通过。候选已完成本地验证，等待 PR/主线 CI 与合入发布。所有测试使用临时数据库与 FakeProvider，没有操作正式代理、Cursor 安装文件或用户会话。

剩余边界：

- 这是通知接收与模型投递修复；不新增 SendToAgent、现代 Await、项目协调器或 Agent Store 功能。
- `record_only` 本轮只落实“不唤醒模型”的语义；原生 `recordSubagentRunCompletion`、`subagent_state` 指针和 Shell/语音状态同步不在本补丁中，不能称为完整原生状态同步支持。
- 新通知在真实 Cursor 的生成、网络投递、原生卡片与真实供应商行为仍待验收；已有 #155 完成一半的结论不变，不测 20 分钟任务。
