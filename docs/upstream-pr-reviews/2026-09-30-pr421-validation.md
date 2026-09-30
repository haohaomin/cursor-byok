# #421 验证与补齐修复

原 PR 不能直接照搬：它修复后续输出项漏字，但在交错事件、迟到终止事件及首次编号晚于文本出现时，会重复输出。已在隔离分支 `codex/verify-pr421` 补齐；尚未合入主分支、发布或执行真实 Cursor UI 测试。

- 基线：个人 fork `3f1b202`（已发布 v1.0.5）。
- 上游：[PR #421](https://github.com/leookun/cursor-byok/pull/421)，作者 kevin9327，验证 head `8c3cedff1a615b16ac963601b8bb964090056104`。
- 查询时上游 PR 为 OPEN、MERGEABLE，4 项 CI 通过；不能以此替代本地行为验证。

## 实测对比

同一套 9 项测试经过回环 HTTP/SSE → 正式 OpenAiResponsesProvider → consume_model_cycle，检查最终文本、工具参数、结束原因和用量。使用本地模拟响应，没有请求真实供应商或改动已安装应用。

| 实现 | 通过 | 失败 | 主要问题 |
| --- | ---: | ---: | --- |
| 当前主分支 | 5 | 4 | 第二个文本项无法补全；交错/迟到项补全失败 |
| 原 PR | 6 | 3 | 切换输出项时清空累计状态，回到旧项时再次输出全文；首次显式编号也会丢失已有前缀 |
| 补齐修复 | 9 | 0 | 按输出项保存累计文本，首次显式编号认领已收到的无编号前缀 |

原 PR 的实际失败值：

- 迟到终止事件：预期 `firstsecond`，实际 `firstsecondfirst`。
- 交错事件：预期 `甲乙尾甲尾乙`，实际 `甲乙甲尾甲乙尾乙甲尾甲乙尾乙`。
- 首个 delta 无编号、后续编号为 2：预期 `hello world`，实际 `hellohello world`；这是原 PR 相对基线引入的回归。

交错和迟到事件属于合成边界测试，不表示已在真实 OpenAI 服务中观测到相同事件序列。

## 改动职责

```text
server/
├── src/provider/openai_responses.rs  # 607 行；仅修改 SSE 文本累计状态，生产差异 +26/-3
└── tests/responses_text.rs           # 211 行；9 项真实 HTTP/SSE 回归测试
```

行数包含注释和空行；生产文件与测试文件分别统计，不含生成代码。

单次响应内，文本累计值以 output_index 分开保存，直到响应结束才释放。流式 delta 仍按到达顺序立即发送，终止快照只补发该项尚缺的后缀。缺少 output_index 的事件沿用当前项；首次编号出现时，之前无编号的前缀归入该项。

不改变请求内容、历史序列化、checkpoint、数据库、工具协议或发布配置。没有重排已经发送的文本；多内容片段用 item.done 完整快照补齐的既有行为保留。原 PR 的三个辅助函数测试已由 HTTP/SSE 行为测试覆盖，没有保留重复的实现层测试。

## 测试覆盖

1. 仅终止快照、文本 → 工具 → 文本；含正常 completed 和完整 item.done 后 EOF 两种结束。
2. 后续项丢失 Unicode 后缀，重复快照不重放。
3. 同一项重复 text.done / item.done 不重放。
4. 多项交错到达，分别补齐自己的后缀。
5. 旧项迟到的 item.done 不重放。
6. 同一项的多个内容片段由 item.done 补齐。
7. 缺少 output_index 的后续事件保持当前作用域。
8. 空消息不影响后面的文本。
9. 首次显式编号认领无编号前缀。

## 最终验证

- `cargo test -p cursor-server --all-targets`：279 通过、0 失败，其中本次专项测试 9 项。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- `cargo fmt --all -- --check`：通过。
- `git diff --check`：通过。

本机编译使用命令级 `SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk`，并通过 `RUSTFLAGS=-C link-arg=-fuse-ld=/Users/renhao/.rustup/toolchains/stable-aarch64-apple-darwin/lib/rustlib/aarch64-apple-darwin/bin/gcc-ld/ld64.lld` 指定链接器；复用主目录 target 缓存，没有修改全局工具链设置。

结论：候选修复已通过服务端验证，仍待真实 Cursor UI 测试及后续合入、发布。清单完成数保持 2 项。

[结构化证据](2026-09-30-pr421-evidence.json) 保存对比结果及日志摘要；原始日志在 `/tmp/cursor-byok-pr421-20260930/`。


## 真实 UI 尝试（受阻，未通过）

2026-09-30 已构建候选服务端，并准备隔离 SQLite、临时 Cursor 工作区和本地 Responses SSE 模拟服务。通过 Cursor 界面打开了 `/tmp/cursor-byok-pr421-20260930/ui/cursor-workspace`，测试代理接通且模型目录请求到达隔离服务。

macOS 界面控制反复出现 ScreenCaptureKit `-3811`、`noWindowsAvailable` 和菜单 `elementHasNoFrame`。之后截图停留在旧画面，无法稳定选择测试模型或提交对话。模拟模型请求数为 0，真实供应商请求数为 0；四个 UI 场景均未取得通过结果。自动测试通过结论不受影响，但不能替代本轮 UI 验证。

已禁用并停止测试服务（53021/53022）及模拟服务（52821），移除临时真实模型配置，恢复正式服务（49588/49642）和 4 个原有模型。Cursor 设置文件与测试前备份逐字节一致。未合入、未发布。

[本次尝试及恢复证据](2026-09-30-pr421-ui-attempt.json)。恢复可靠界面控制后，从选择测试模型继续。


## 真实供应商连通性补测

第二次尝试真实 Cursor UI 时，菜单输入仍被 `elementHasNoFrame`、`noWindowsAvailable` 和 ScreenCaptureKit `-3812` 阻塞。通过窗口菜单切换到独立 Cursor Agents、恢复窗口尺寸后仍无法提交对话。

为推进独立验证，通过隔离服务的正式 Control API 模型测试入口，用既有 `gpt-5.6-sol` 供应商配置访问 `/v1/responses`（只在临时模型配置中切换协议）。实际返回数字 1～120，与预期字符串完全一致，无缺失或重复。耗时 16044 ms，首个有效响应 2557 ms，真实计费用量输入 30 / 输出 243 Token；SQLite 记录 1 次 `openai-responses` 调用、HTTP 200、completed / stop、无错误、140 个流事件。

该补测是实际供应商连通性测试，未经过 Cursor UI，也没有多轮追问；不能代替四个界面场景。详见 [真实供应商证据](2026-09-30-pr421-live-connectivity.json)。

补测结束已移除临时真实模型配置、停止测试服务并恢复正式服务（4 个模型）；Cursor 设置与本轮开始前逐字节一致，测试端口均关闭。
