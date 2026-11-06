# 上游 PR #211：补齐 Grep 结果上下文

## 结论与来源

当前 Rust 实现已经保留 Grep 的匹配正文、文件列表、逐文件计数与行截断标记，也有完成边界的 32 KiB 回放限制。仍有三处缺口：分页的 `head_limit_applied` / `offset_applied` 被丢弃，工作区与活动编辑器的结果混在一起，计数和正文模式缺少部分总量信息。空的后续页因此可能变成“没有匹配”，同名相对路径也无法区分来源。

参考上游 [PR #211](https://github.com/leookun/cursor-byok/pull/211)，本轮固定 head 为 `d280adae51f691aaa1eb44240416f55d90084d05`。该 SHA 与历史清单的旧 SHA 不同；已重新检查正文、两个文件的完整补丁及测试、评论、review 和 checks。当前仍为草稿，未提供可见 CI 结论；作者自述测试不作为本 fork 的验证证据。

在 Rust 工具结果层独立实现剩余缺口。当前工具链没有旧 Go 的 Ls 入口，因此不移植旧目录树实现，也不把本次标为新增 Ls 支持。Grep 和共享该协议结果的 Glob 继续使用已有大小限制，没有引入第二套截断机制。

实现分支：`codex/pr211-grep-result-context`。最终回归基线为 main `4e90dce850219947e945093de7ee246e85514198`，已包含标题修复 PR #24。本次没有修改插件、桌面 UI 或发版配置。

## 结构与行为

```text
server/
├── src/cursor/tools/tool_call_result/exec/
│   ├── output.rs       # 432 行；分派 Exec 结果，保留其余工具转换
│   ├── grep.rs         # 新增 162 行；Grep 范围、分页与数据格式化
│   └── mod.rs          # 将转换结果交给统一 ToolCompletion
├── src/cursor/tools/tool_call_result/gate.rs # 既有完成边界大小限制，未修改
├── src/run/            # 既有工具结果提交和后续模型循环，未修改
├── src/store/          # 既有不可变 checkpoint 存储，未修改
├── src/model/          # 既有统一 provider 历史投影，未修改
└── tests/grep_results.rs # 新增 266 行；实际 Exec 转换及存储回放回归
```

行数实测，包含空行与注释；生产文件行数不含测试或生成代码。原 `output.rs` 中的 Grep 格式化函数移入 `grep.rs` 并补齐信息，旧函数删除，没有保留两条实现。

```text
Cursor GrepResult（文件 / 计数 / 内容）
  → Exec ID 对应的待完成工具
  → grep.rs：编辑器标签 → 排序后的工作区标签 → 分页说明 → 数据
  → ToolCompletion：现有大小限制，只在完成时执行一次
  → 运行层保存 ToolResult / checkpoint
  → 统一历史投影 → 下一次 provider 调用
```

- 分页说明包含已返回数量、总量、head limit、offset，以及客户端或 ripgrep 截断原因。
- 分页与总量说明置于对应正文之前，避免长正文截断后丢失这部分信息；超限结果仍有原有 byte-limit 通知。
- 内容模式继续保留匹配/上下文行分隔符、行号及单行截断标记。
- 工作区按名称排序，活动编辑器单独标识；同名相对路径保留所属范围。
- 空的后续页保留分页信息；没有限制且确实为空的搜索仍返回明确的无匹配结果。
- 只改变新结果，不重新生成已保存的 canonical 消息；checkpoint 恢复后旧结果保持字节内容和投影前缀。

## 验证

新增五项行为测试，使用真实 `CursorToolRuntime → codec::client_event → ToolCompletion` 路径。在失败基线 `b6936e8` 上，修复前 1 项通过、4 项失败，分别暴露分页、计数/正文信息、范围标签及长结果中的元数据缺失。修复后五项全部通过；接入最新 main `4e90dce` 后再完成全部服务端回归。

| 检查 | 结果 |
| --- | --- |
| `cargo test --locked -p cursor-server --test grep_results` | 5 项通过 |
| `cargo clippy --locked -p cursor-server --all-targets -- -D warnings` | 通过 |
| `cargo test --locked -p cursor-server --all-targets` | 503 项通过、0 失败；13 项需 Deno 的插件测试、1 项需缓存 Semble 模型的测试按原配置忽略 |
| `cargo fmt --all -- --check`、`git diff --check` | 通过 |

完整服务端检查包含现有 prefix stability、checkpoint recovery、compaction、工具执行及重试回归。

大结果用例经过现有 32 KiB 限制，随后保存并读取 SQLite checkpoint，再投影为 provider 历史；追加下一条消息后，之前的投影保持完整结构前缀。测试使用合成 Cursor protobuf 和隔离数据库，没有声称完成真实 Cursor UI、真实远程文件搜索或原作者 Windows 环境验收。

## 同时核对的重叠候选

上游 [PR #210](https://github.com/leookun/cursor-byok/pull/210) 的当前 head 为 `f9792dc1fb88b50c2557a89d84a62cbe2c03a5a1`。当前 `run/engine.rs`、`run/model_retry.rs` 已在同一 Run 中提供有限重试、等待期间的取消和新输入处理；`error_lifecycle`、`tool_presentation`、`issue172_repro` 已覆盖失败后的内容保留和工具呈现。其策略并不等同于原 PR 的“零输出、最多三次”方案，因此本轮不重复引入 Go 重试层，也不宣称整个原 PR 已移植。
