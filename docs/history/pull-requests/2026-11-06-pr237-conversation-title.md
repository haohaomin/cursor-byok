# 上游 PR #237：会话标题与元数据

## 结论与来源

当前 Rust 服务没有本地 `NameTab` 路由；`UpdateConversationMetadata` 原先返回成功，但不保存请求。本次在现有服务、模型路由和 SQLite 存储内补齐这两条路径。

参考上游 [PR #237](https://github.com/leookun/cursor-byok/pull/237)，固定 head 为 `ef060ea576d798d8de1cb6d5ab2776853ba1aa36`。已检查该版本完整补丁、评论与 review；没有可用的上游测试结论。旧 Go forwarder/host 实现不能直接移植，因此只采用独立短标题请求、允许元数据早于 Run 到达的行为目标，没有恢复旧模块或启发式备用标题。

本次基线为 fork main `b6936e885db616e3f7b658d2c03bb5722f55675d`。分支为 `codex/issue237-conversation-title`，尚未发布桌面版本。

## 当前结构与执行路径

```text
server/
├── src/api/cursor/handlers.rs              # 注册标题生成和元数据更新 RPC
├── src/local_app/proxy.rs                  # 将 NameTab 纳入本地代理分流
├── src/cursor/protocol/proto.rs            # 解码 NameTab 所需的最小字段集合
├── src/cursor/services/conversation_title.rs # 217 行；独立标题调用及元数据入口
├── src/model/configuration.rs             # 标记辅助调用不继承聊天推理参数
├── src/provider/router.rs                 # 保留模型路由，隔离聊天额外参数
├── src/store/conversation_metadata.rs     # 90 行；原子更新名称与工作区路径
└── migrations/0012_add_conversation_metadata.sql # 新增独立元数据表
```

行数实测，包含空行与注释；两个列出行数的生产文件不含测试与生成代码。测试分别在 `src/api/cursor/title_tests.rs` 与 `src/store/conversation_metadata_tests.rs`。

```text
NameTab 请求
  → 本地 Cursor token？
    ├─ 否 → 原有官方代理
    └─ 是 → 首条非 AI 的非空文本（最多 2,000 字符）
            → 已选择的 Commit 模型，或现有模型目录的默认模型
            → 新建独立模型调用（独立 ID、无工具、128-token 输出预算）
            → 消费事件直到完成；最长 15 秒，遵守更短的客户端 deadline
            → 清理输出，返回最多 48 字符的标题

UpdateConversationMetadata 请求
  → 本地 Cursor token？
    ├─ 否 → 原有官方代理
    └─ 是 → 写协调锁 + SQLite BEGIN IMMEDIATE
            → 按 only_set_name_if_empty 原子更新，规范化并去重路径
            → 独立元数据表提交 → 成功响应
```

标题调用不读取或修改目标会话历史，不建立 Agent Run，不继承聊天模型的思考配置及自由格式请求参数；错误、拒绝、空结果、工具调用、超时和缺少完成事件均返回错误。成功、失败及超时退出会取消调用的 token。

元数据可以在首个 Run 前保存，因此不依赖会话表外键，也不创建空 checkpoint。已有 active run、checkpoint 与消息保持不变。重复设置有名称保护；仅改名称的请求不会清空已知路径，因为 protobuf repeated 字段无法区分缺省与空数组。

原有兼容处理器中的空成功实现被删除；没有新增第二条会话运行或历史持久化路径。

## 复现与验证

先运行新增协议回归，再实现修复：

- 本地 `NameTab` 原先落入代理并返回 400，未满足预期的本地 200 响应。
- 元数据请求原先返回成功，但库中没有对应元数据表或保存结果。
- 初版直接模拟 Provider 的测试通过后，进一步接入真实 `ProviderRouter` 和本地 HTTP 服务，发现 Anthropic 默认思考配置仍被带入请求。补齐参数隔离后，该回归通过；保存的 `max_tokens=30000` 与 `thinking=adaptive` 也不能覆盖短标题请求。

最终验证：

| 检查 | 结果 |
| --- | --- |
| 新增标题与协议测试 | 7 项通过：独立调用、模型选择、输入边界、早到元数据、官方转发、失败处理、超时及 HTTP 请求参数 |
| 新增存储测试 | 2 项通过：从 0011 升级、重启持久化、现有活动历史不变、跨 Store 并发更新、新库约束 |
| `cargo test --locked --workspace --all-targets` | 563 项通过，0 失败；13 项需 Deno 的插件测试、1 项需缓存 Semble 模型的测试按原配置忽略 |
| `cargo clippy --locked --workspace --all-targets -- -D warnings` | 通过 |
| `cargo fmt --all -- --check`、`git diff --check` | 通过 |

全量测试覆盖现有 checkpoint、前缀投影、压缩、取消、工具结果和桌面 Rust 目标；没有仅以标题单元测试代替公共模型路由回归。

这些结果来自隔离数据库、模拟事件流和本地 HTTP 服务，未使用真实 provider 账号，未验收真实 Cursor 自动改名、远程 SSH 或安装包 UI。真实模型的输出质量与不同服务的默认推理行为仍需要实机验证；此记录不宣称上述场景全部完成。
