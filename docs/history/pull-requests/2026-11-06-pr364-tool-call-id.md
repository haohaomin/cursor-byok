# 上游 PR #364：长工具调用 ID 的选择性重实现

> 日期：2026-11-06。本文记录当前 fork 的独立实现与验证边界；上游仓库已归档，不能把上游 PR 的 CI 或作者声明当作本 fork 的测试结果。

## 结论

上游 [PR #364](https://github.com/leookun/cursor-byok/pull/364) 将工具调用 ID 的全局上限从 64 放宽到 256。该方案不能直接合入当前 fork：部分 provider 仍拒绝超过 64 字节的 ID，上游讨论也明确指出全局放宽会导致 OpenAI 请求失败。

当前 fork 采用小范围重实现：

- canonical conversation 和持久化消息继续保存原始工具调用 ID；
- `NormalizedProvider` 只修改 provider 请求的 `ProjectedMessage` 副本；
- 不超过 64 字节的 ID 保持原值；
- 超过 64 字节的 ID 使用完整 ID 的 SHA-256 十六进制值，结果固定为 64 个 ASCII 字节；
- assistant tool call 与对应 tool result 使用同一个稳定映射，不再因共享前 64 字节而碰撞。

这样保留了 OpenAI、Anthropic 及兼容代理共同可接受的 64 字节边界，同时解决长 ID 被简单截断后无法匹配工具结果的问题。实现没有把 provider 特例散落到各适配器，仍由统一的 provider 规范化层处理。

## 代码边界

```text
server/src/model/inference.rs       # 统一 provider 投影中的 ID 规范化与 4 项单元回归
server/src/provider/normalize.rs    # 既有入口；把请求历史副本交给统一规范化函数
server/src/provider/*.rs            # 未修改；继续接收规范化后的 provider 历史
```

`inference.rs` 当前 174 行，包含注释、空行和测试；生产逻辑约 20 行，测试约 100 行。行数不含生成代码。canonical history、checkpoint 和数据库 schema 均未修改。

## 参考实现与避免重复

- 上游原补丁：[leookun/cursor-byok#364](https://github.com/leookun/cursor-byok/pull/364)，只把常量 64 改为 256。
- [Roo Code 的 OpenAI provider](https://github.com/RooCodeInc/Roo-Code/blob/main/src/api/providers/openai-native.ts) 保持 OpenAI 工具调用 ID 不超过 64。
- [pi 的 Responses 兼容性记录](https://github.com/earendil-works/pi/issues/1022)也记录了 Responses provider 的 64 字符上限。

因此没有重复照搬 256 上限，而是把当前 fork 已有的截断逻辑改成稳定、无前缀碰撞的映射。

## 验证

使用独立构建目录和已安装的 macOS 26.5 SDK：

| 检查 | 结果 |
| --- | --- |
| `cargo test --locked -p cursor-server --lib model::inference::tests` | 4 通过、0 失败 |
| `cargo test --locked -p cursor-server --lib` | 255 通过、14 忽略、0 失败 |
| `cargo test --locked -p cursor-server --test prefix_stability` | 13 通过、0 失败 |
| `cargo test --locked -p cursor-server --test checkpoint_recovery` | 4 通过、0 失败 |
| `cargo test --locked -p cursor-server --test compaction` | 9 通过、0 失败 |
| `cargo test --locked -p cursor-server --all-targets` | 退出码 0；所有执行中的目标通过，既有需 Deno/真实模型的测试仍按项目规则忽略 |
| `cargo clippy --locked -p cursor-server --all-targets -- -D warnings` | 通过 |
| `cargo fmt --all -- --check` | 通过 |
| `git diff --check` | 通过 |

新增回归覆盖：短 ID 保持不变、长 ID 在 assistant/tool result 间一致、相同前缀的不同长 ID 不碰撞，以及多字节输入仍被限制为 64 字节。

## 未覆盖范围

本轮没有使用真实 OpenAI、Anthropic 或代理账号发送请求，也没有修改安装包。真实 provider 对长 ID 的端到端响应仍应在 PR CI 通过后单独验证；本实现首先保证请求序列化边界和历史中的 ID 配对关系。
