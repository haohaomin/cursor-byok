# 上游 PR #345 选择性移植记录

## 结论

当前 fork 已经具备上游 PR #345 的主要能力：规则的本地持久化、增删改查、离线同步，以及将本地规则合并到会话上下文。本次只补齐当前代码缺失的 `FetchRelevantKnowledgeForConversation` 协议入口，没有复制上游旧的 handlers、prompting 或存储实现。

## 变更范围

- 在当前 Cursor API 路由中注册 `FetchRelevantKnowledgeForConversation`。
- 在本地代理分流表中标记该路径，确保请求留在本地服务。
- 从现有 `KnowledgeService` 读取本地规则并返回 Cursor 需要的 protobuf 项目。
- 支持非负 `limit`；会话内容、Git origin 和 tagged filenames 只用于协议解码，不参与筛选，因为 fork 的规则是全局规则。
- 复用现有规则存储和 `merge_local_rules`，避免重复注入及旧架构回退。

上游参考：[#345](https://github.com/leookun/cursor-byok/pull/345)，固定 head `76884f15e9c9e9c3ced0039761fa17fd0d8bdac2`。上游补丁基于旧路径，不能直接合并到当前 Rust/Tauri 架构。

## 验证

已通过：

```text
cargo test --locked -p cursor-server --test knowledge_rules
cargo test --locked -p cursor-server --lib local_app::proxy
cargo test --locked -p cursor-server --all-targets
cargo clippy --locked -p cursor-server --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

新增回归覆盖：本地规则在不同 Git origin、会话字节和 tagged filename 下仍返回，并遵守 `limit`；完整服务端测试和 Clippy 也通过。

这些是本地协议和服务端验证，尚未声称完成真实 Cursor UI、账号或远程 SSH 端到端验收。
