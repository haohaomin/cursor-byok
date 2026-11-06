# 上游 PR #378 选择性移植记录

## 结论

当前 fork 已经包含上游 #378 中的多项相关能力，包括压缩后的提交屏障、上下文重新估算、重复自动压缩以及本地回退摘要。本次只补齐仍然缺失且可以独立验证的行为：自动压缩摘要模型永久挂起时，运行不能无限等待。

没有复制上游旧架构中的完整 compaction recovery 重构，也没有重复移植当前主线已经覆盖的重复压缩和工具结果投影逻辑。

## 变更范围

- 为自动压缩摘要模型增加 30 秒总 deadline。
- deadline 到期后取消摘要流，使用现有 durable fallback summary。
- 继续发布压缩完成事件、持久化 checkpoint，并进入后续正常模型调用。
- 保持用户取消、工具结果和正常摘要响应的既有路径。

上游参考：[#378](https://github.com/leookun/cursor-byok/pull/378)，固定 head `1b29c79f1baac8295141f7e96e429b803980d2a1`。该 PR 基于较旧的运行架构，完整移植会重复当前 fork 已有实现并扩大风险范围。

## 验证

先在未修改基线运行新测试，因摘要 provider 永久 pending 而在测试超时失败；加入 deadline 后通过。最终通过：

```text
cargo test --locked -p cursor-server --test compaction stalled_automatic_compaction_falls_back_and_continues -- --nocapture
cargo test --locked -p cursor-server --test compaction -- --nocapture
cargo test --locked -p cursor-server --test checkpoint_recovery -- --nocapture
cargo test --locked -p cursor-server --test tool_round one_run_can_auto_compact_again_after_more_tool_output -- --nocapture
cargo test --locked -p cursor-server --all-targets
cargo clippy --locked -p cursor-server --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
```

新增回归实际等待约 30 秒，确认摘要流挂起后仍能生成本地摘要、写入存储并继续下一次模型调用。

这些是服务端和协议层验证，未声称完成真实 Cursor UI、账号或远程 SSH 端到端验收。
