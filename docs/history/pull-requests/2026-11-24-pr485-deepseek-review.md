# PR #485：DeepSeek 跨协议续写初审

日期：2026-11-24。来源：[lawyer112 的上游 PR #485](https://github.com/leookun/cursor-byok/pull/485)，固定 head `49e29e3756e1d2c0589281ad0527f7259b9e6887`。已读完整相关补丁；本项仅初审，尚未新增失败复现或修改生产代码。

## 当前实现与原补丁

```text
server/src/provider/
├── openai_responses.rs   读取 reasoning 文本事件，并保存原生 reasoning 输出项
└── openai_chat.rs        回传原生 Chat reasoning_content
```

当前 Responses 适配器已支持 `response.reasoning_text.delta/done`，并在原生回放状态中保留 `content`、`summary`、`encrypted_content` 等字段。Chat 只回放 `openai_chat` 来源的 `reasoning_content`，会过滤空字符串。

原补丁对名称包含 deepseek 的模型改用通用 `thinking` 补齐 Chat 字段，工具调用无思考文本时补空字符串。通用展示字段可能来自另一协议的摘要，不能直接认定为完整原生思考内容。

## 官方协议核对

- [Thinking Mode](https://api-docs.deepseek.com/guides/thinking_mode/) 要求携带工具的 Chat 请求完整回传此前的 reasoning_content，包括没有工具调用的轮次。
- [Responses API](https://api-docs.deepseek.com/guides/responses_api/) 区分原生明文 reasoning.content 与 summary、encrypted_content；不能把后两者当作同一份输入。
- [Tool Calls](https://api-docs.deepseek.com/guides/tool_calls/) 对 Chat 中插入非模型产生的工具调用另有限制，因此补一个字段不等于跨协议工具会话已被真实服务接受。

## 下一步与验收边界

1. 先用隔离 HTTP 服务和生产 Provider 路径复现：原生 Responses 明文 reasoning → SQLite/检查点恢复 → Chat 工具续写。
2. 单独验证原生 Chat 返回显式空 reasoning 的保存和回放，不以空串替代未知或缺失的完整内容。
3. 保持原生 Chat 回放优先，避免向不支持该字段的模型发送其他协议的摘要或加密内容。
4. 确定性协议测试通过后再核验实际网关；没有真实请求证据前，不宣称已解决所有 DeepSeek 400 或工具续写问题。

本次没有使用模型 API 凭据、访问用户对话或操作原生 Cursor；不会把原作者的实机声明计为本 fork 验收。
