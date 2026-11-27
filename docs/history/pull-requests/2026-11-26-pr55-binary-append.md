# PR #55：接收 BidiAppend 二进制载荷

来源：[本 fork PR #55](https://github.com/haohaomin/cursor-byok/pull/55)，评估 head `26fa33de2c8da9103a7d2121ec53dfc224442333`；关联 [Issue #54](https://github.com/haohaomin/cursor-byok/issues/54)。基线为 `main` 的 `9001073d03e241a2e37d998ad41d912993cb162f`。已读取正文、全部差异、评论、review 与行内讨论；原 PR 的 Sourcery 通过不能替代本轮执行验证。

## 问题与采用范围

现有解码器拒绝所有非空 `data_binary`。合法 `AgentClientMessage` 经 gzip/protobuf HTTP 入口时返回 400，模型尚未执行。模型配置的连通性测试不走该入口，因此可以出现“模型测试成功，但 Agent 请求失败”。

采用作者的二进制载荷解码目标和测试，保留作者提交历史，再补齐以下边界：

- 明确选择 `data` 或 `data_binary` 中唯一非空的字段。两个字段同时存在、两个都空、请求 ID 缺失或为空均拒绝；解码失败不改用另一个字段重试。
- 二进制内容直接借用请求中的字节；十六进制字段只在自身被选中时解码。现有捕获记录及 HTTP 子代理测试使用 `data`，没有证据证明它已失效，因此按两种明确的协议表示处理。
- 两种表示共用 protobuf 解码和脱敏诊断，只记录请求 ID、序号、载荷长度及解码错误，不输出消息正文。
- 原 head 混入的 180 秒自动压缩超时修改在整合时撤回。超时行为由 [PR #56](https://github.com/haohaomin/cursor-byok/pull/56) 单独审查；本次最终差异不修改 `run/engine/compaction.rs`。

```text
server/
├── src/api/cursor/bidi.rs     # 选择并解码唯一载荷，交给已有 transport
└── tests/connect_wire.rs     # gzip HTTP → Agent → 回复/KV 确认 → 检查点与终止
```

新输入仍进入同一 transport/run 生命周期，不创建旁路执行器，也不改变顺序、去重、检查点和持久化规则。

## 失败复现与验证

1. 基线加 HTTP 回归：`binary_bidi_append_runs_provider_and_returns_checkpoint` 失败，返回 `400 / BidiAppend data_binary is not part of the captured protocol`，预期为 200。
2. 原 PR 候选加双字段回归：4 项解码测试中 3 通过、1 失败；合法十六进制字段会掩盖同时存在的二进制字段，包括无效二进制内容。
3. 补全后：6 项解码测试、7 项 Connect wire 测试、8 项子代理准入测试全部通过。新 HTTP 回归验证 gzip 二进制 Run、完整回复、二进制 KV 确认、检查点、正常终止、只调用模型一次及释放会话。
4. `cargo test --locked -p cursor-server --all-targets`：35 个目标合计 560 通过、0 失败、14 忽略。Clippy 全目标 `-D warnings`、格式检查和差异检查通过。

本机首次编译遇到既有 SQLx 宏 dylib 的 `mis-aligned LINKEDIT string pool`，使用既有命令行覆盖 `--config 'profile.dev.package.sqlx-macros.strip="none"'` 后解除，未修改仓库构建配置。

本轮使用隔离 SQLite 与模拟 provider；作者声称已在 Cursor 3.29.9/3.32.4 实测，属于作者证据。本轮未重复真实 Cursor UI/供应商验收，不能据此认定历史 Windows Agent、远程 SSH 或 #155 后台审批问题全部解决。

## 审查与合入

整合提交 `d062c320e1de049b2c1378e59d1e5f7e4f29bf26` 经 [PR #59](https://github.com/haohaomin/cursor-byok/pull/59) 合入 main，合并提交 `438ec8ffee1ff4e43827e08486006748eb7d6333`。该 head 的 [PR CI](https://github.com/haohaomin/cursor-byok/actions/runs/45812933105) 与 [push CI](https://github.com/haohaomin/cursor-byok/actions/runs/45812876791) 的 Linux、Windows、macOS、前端检查全部通过。GitHub 已将原 PR #55 标记为 merged，并自动关闭其关联 Issue #54；这不扩大上述实机验收范围。

Sourcery 检查通过，但提出了两项意见，逐项判断如下：

- 删除十六进制字段：未采纳。现有捕获协议及子代理 HTTP 测试仍使用该字段，没有证明它已经过时。本次不存在失败后尝试另一格式的回退。
- 限制超大请求：记录为共用入口的独立待办。既有 `handlers.rs` 已使用 `DefaultBodyLimit::disable()` 和 `to_bytes(body, usize::MAX)`，解压与外层 protobuf 读取先于本次载荷选择，十六进制路径也具有同一风险。本次不以任意内部载荷阈值冒充完整防护；后续需统一限定读取/解压边界，并验证合法大图和长历史请求。

## 发布

修复已随 [v1.2.8](https://github.com/haohaomin/cursor-byok/releases/tag/v1.2.8) 发布。主线 CI、四平台发布、20 项资产哈希/大小、7 个签名和三个更新清单均通过，[发布核验](../releases.md#v128)。本轮未安装新包到用户机器。

## 后续：HTTP 大请求上限

2026-11-27，本报告独立记录的共用读取/解压入口缺口已通过 [PR #63](https://github.com/haohaomin/cursor-byok/pull/63) 修复并随 v1.2.11 发布，覆盖大小边界、大历史和 PNG、探针以及流式转发；见 [复现与验证](../issues/2026-11-27-cursor-request-body-limit.md)。原 #54 的真实 Cursor UI 验收边界仍保留。
