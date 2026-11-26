# PR #485：DeepSeek 跨协议续写复核与选择性修复

日期：2026-11-26。来源：[lawyer112 的上游 PR #485](https://github.com/leookun/cursor-byok/pull/485)，固定 head `49e29e3756e1d2c0589281ad0527f7259b9e6887`。本 fork 没有整体合入上游 PR，而是保留可验证的协议边界，独立修复 DeepSeek 的跨 Responses/Chat 续写。

## 当前实现与原补丁

```text
server/src/provider/
├── openai_responses.rs   读取 reasoning 文本事件，并保存原生 reasoning 输出项
└── openai_chat.rs        回传原生 Chat reasoning_content
```

当前 Responses 适配器已支持 `response.reasoning_text.delta/done`，并在原生回放状态中保留 `content`、`summary`、`encrypted_content` 等字段。修复后，完成快照中的完整原生 reasoning 项会替换流式基线，避免网关只在最终快照发送 reasoning 时丢失或重复。Chat 仍优先回放自身 `openai_chat` 状态；当模型 ID 的最后一段以 `deepseek-` 开头时，才会把 Responses 回放状态中完整的明文 `reasoning_text` 拼成 `reasoning_content`。

Chat 流解析也会保存服务端明确返回的空 `reasoning_content`/`reasoning`，因此工具调用轮次的显式空值可以经过检查点和重启回放；缺失字段、摘要、加密内容和不完整明文不会被伪造为空值或跨协议发送。

## 失败复现与修复证据

隔离 HTTP 服务在修复前确认了三个失败点：Responses 明文 reasoning 经过 SQLite/检查点恢复后续接 Chat 时缺少 `reasoning_content`；仅在 `response.completed` 快照出现的 reasoning 同样丢失；Chat 返回显式空 reasoning 时，持久化状态被过滤。修复后的同一组测试要求隔离服务严格核对每个 assistant 的 reasoning 内容，并通过数据库关闭、重开、检查点解码和下一轮请求。

新增回归覆盖：

- Responses 明文 reasoning → 检查点恢复 → DeepSeek Chat 工具续写；
- reasoning 只出现在最终响应快照时的回放；
- Chat `reasoning_content: ""` 和 `reasoning: ""` 的保存与回放；
- 摘要、加密内容、空内容、不完整内容、非 DeepSeek 模型均不跨协议伪造字段。

验证结果：`cargo fmt --all -- --check` 通过；`cargo clippy --locked -p cursor-server --all-targets -- -D warnings` 通过；`cargo test --locked -p cursor-server --lib` 为 309 通过、14 忽略；`cargo test --locked -p cursor-server --all-targets` 的服务端单元与集成目标全部通过，其中 `responses_tool_history` 为 6 通过。

## 官方协议核对

- [Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode/) 要求携带工具的 Chat 请求完整回传此前的 reasoning_content，包括没有工具调用的轮次。
- [Responses API](https://api-docs.deepseek.com/guides/responses_api/) 区分原生明文 reasoning.content 与 summary、encrypted_content；不能把后两者当作同一份输入。
- [Tool Calls](https://api-docs.deepseek.com/guides/tool_calls/) 对 Chat 中插入非模型产生的工具调用另有限制，因此补一个字段不等于跨协议工具会话已被真实服务接受。

## 验收边界

1. 本次验证的是生产 Provider、HTTP 请求形状、SQLite/检查点恢复和前缀稳定性；没有使用模型 API 凭据、访问用户对话或操作原生 Cursor。
2. 这项修复只在模型 ID 最后一段以 `deepseek-` 开头时跨 Responses → Chat 发送完整明文 reasoning；其他模型仍保持原有字段隔离。
3. 摘要或加密 reasoning 没有可安全转换成 DeepSeek `reasoning_content` 的完整明文，因此会被丢弃并由网关决定是否接受该历史；本记录不宣称解决所有真实网关的 400。
4. 真实 DeepSeek 账号、Cursor 原生 Agent UI 和其他 PR 的子代理路径仍需单独验收。
