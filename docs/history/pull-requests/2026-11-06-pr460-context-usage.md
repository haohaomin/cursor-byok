# 上游 PR #460：缺失 usage 时估算上下文用量

日期：2026-11-06

上游 PR：[leookun/cursor-byok#460](https://github.com/leookun/cursor-byok/pull/460)，上游提交 `98d57a0a605843a9f7d1a8af441772935520d831`。

## 采用范围

本 fork 在 `origin/main` 上重新实现了同一行为：

- checkpoint 没有模型窗口或有效历史窗口时，使用 200,000 token 作为估算上限；已有模型窗口仍优先。
- provider 成功发出 `Done`、但整个流没有 `Usage` 事件时，按 prompt 和历史估算输入用量，并写入 `llm_calls`。
- 估算 usage 在 recorder 已记录完成状态后持久化，但在下游收到 `Done` 前发送，保持现有事件顺序。
- provider 已返回真实 usage 时原样保留；异常流、超时和取消不会生成成功用量。

插件目录只增加了测试用的 `PluginRegistry::for_test` 构造器，未改变插件生产运行逻辑。

## 验证

以下命令在 macOS 使用全新的临时 target 目录执行：

```text
cargo fmt --all -- --check
cargo test --locked -p cursor-server provider::router::tests -- --nocapture
cargo clippy --locked -p cursor-server --all-targets -- -D warnings
cargo test --locked -p cursor-server --all-targets -- --nocapture
```

结果：格式和 Clippy 通过；provider 定向测试 6 项通过；服务端全量 256 项通过、14 项忽略、0 项失败。新增集成测试覆盖：

1. provider 不返回 usage 时生成并持久化估算值；
2. 真实 usage 返回时不重复生成估算值；
3. 估算值持久化时调用状态已经是 `completed`；
4. 未知窗口、零历史窗口和已配置窗口的优先级。

## 边界

200,000 是缺少模型元数据时的保守估算上限，不代表供应商真实上下文容量，也不代表计费 token。真实 Cursor UI、真实供应商账单和插件模型运行未作为本次验收依据。
