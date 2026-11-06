# 上游 PR #375：ResumeAction 上下文与继续执行

> 来源：[leookun/cursor-byok#375](https://github.com/leookun/cursor-byok/pull/375)，固定 head `50620d9ac5f31b8d57c4e56a05e4f7bfe88c0df8`。本记录对应个人 fork 分支 `codex/issue375-resume-progress`，不是上游原补丁的直接合并。

## 结论

已按当前 fork 的 Rust 架构选择性重实现，尚未合入 `main`。上游补丁使用已经退出的 `server/src/cursor/request/` 路径；当前实现落在 `server/src/cursor/compile/`，只保留 ResumeAction 的实际行为：请求上下文进入恢复请求；没有待处理工具轮次时追加隐藏的继续提示；有待处理工具轮次时先恢复工具，再调用模型。

```text
AgentRunRequest(ResumeAction)
  │
  ├─ compile/context.rs
  │    读取 ResumeAction.request_context，并合并同步上下文
  │
  ├─ compile/run.rs
  │    解码 pending_tool_calls
  │    以 transport request_id 编译稳定的 resume 上下文
  │
  ├─ compile/break_messages.rs
  │    request-context:resume:<request_id>
  │    resume-prompt:<request_id>（没有待处理工具时）
  │
  └─ RunEngine
       有工具轮次：先执行恢复的工具
       无工具轮次：直接进入下一次模型调用
```

## 与上游补丁的关系

上游 PR 涉及 5 个旧路径文件，其中 `request/prepare.rs` 和 `request/runtime.rs` 负责恢复消息编译。当前 fork 的对应边界如下：

| 上游职责 | 当前实现位置 | 处理方式 |
| --- | --- | --- |
| `ResumeAction.request_context` | `server/src/cursor/compile/context.rs` | 增加 ResumeAction 分支，复用现有上下文 hydration |
| Resume 上下文与隐藏提示 | `server/src/cursor/compile/break_messages.rs` | 提取 `compile_resume_messages`，复用内容去重规则 |
| 请求准备和 pending tool round | `server/src/cursor/compile/run.rs` | 显式识别 ResumeAction；保持现有 checkpoint 解码和 RunAction.Resume |
| 活跃 Bidi 会话中的 ConversationAction | `server/src/cursor/conversation/runtime.rs` | 仅更新注释；该路径仍是独立的运行时 dispatcher，不由本 PR 扩大范围 |

Resume 消息使用稳定的 transport `request_id` 生成身份。重复请求会由 checkpoint 的 append-once 和内容比较复用已有消息；不会因为每次 `run_id` 变化而追加重复上下文。待处理工具轮次存在时不追加 `<resume>`，避免在工具恢复前插入新的模型指令。

## 验证

验证均在分支最终代码上执行，目标目录为 `/private/tmp/cursor-byok-issue375-target-20261106`：

| 检查 | 结果 |
| --- | --- |
| ResumeAction 聚焦回归 | 通过；请求上下文到达第二次模型调用，`request-context:resume:explicit-resume` 和 `resume-prompt:explicit-resume` 各只出现一次 |
| `conversation_delivery` | 18 通过 |
| `checkpoint_recovery` | 4 通过；待处理工具轮次先于模型调用恢复 |
| `compaction` | 9 通过；显式 Resume 使用稳定隐藏提示，通用 `runtime:continue` 行为仍有底层测试覆盖 |
| `interrupt` | 23 通过 |
| `cargo test --locked -p cursor-server --all-targets` | 库 251 通过、14 忽略；所有集成目标通过 |
| `cargo clippy --locked -p cursor-server --all-targets -- -D warnings` | 通过 |
| `cargo fmt --all -- --check`、`git diff --check` | 通过 |

## 验收边界

- 尚未进行真实 Cursor 客户端或真实供应商请求验收；本次证据是服务端 transport、checkpoint 和 fake provider 的可控回归。
- 没有实现活跃 Bidi Run 内 `AgentClientMessage::ConversationAction` 的新 dispatcher；这属于另一条生命周期，避免把两个入口混在一起。
- 没有修改插件、桌面 UI、数据库 schema 或发布配置。

