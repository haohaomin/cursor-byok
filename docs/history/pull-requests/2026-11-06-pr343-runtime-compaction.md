# #343 运行时压缩提示词选择性实现

日期：2026-11-06。上游参考：[leookun/cursor-byok#343](https://github.com/leookun/cursor-byok/pull/343)；个人 fork：[haohaomin/cursor-byok#17](https://github.com/haohaomin/cursor-byok/pull/17)。

## 结论

当前主线把手动压缩的提示词交给 `PromptCompiler`，自动压缩却在 `RunEngine` 中使用硬编码字符串。这个 PR 让两条路径都在每次压缩请求时读取同一个可选文件：

```text
~/.cursor-byok-v3/prompts/compaction.md
```

文件不存在或只包含空白时使用内置提示词；文件不是有效 UTF-8 或无法读取时返回带文件路径的配置错误。`{{FAKE_MODEL_NAME}}` 仍由编译器替换，其他 Prompt 模式不读取该文件。

上游 PR 正文还描述了自动创建文件和桌面 Config 页面，但其当前实际提交只包含 4 个服务端文件。本次只移植与当前代码结构对应的服务端行为，不把未实现的桌面页面记为完成。

## 当前执行路径

```text
server/src/app.rs (209 行；启动依赖组装)
    └─ PromptCompiler::with_compaction_prompt_path
       └─ server/src/config.rs (216 行；数据目录与文件读取)

手动 /summarize
    └─ server/src/cursor/compile/run.rs
       └─ PromptCompiler::prompt_spec(Mode::Compaction)
          └─ 每次读取 compaction.md → PromptSpec

自动上下文压缩
    └─ server/src/run/engine.rs (1124 行；RunEngine)
       └─ server/src/run/compaction.rs (528 行；压缩策略)
          └─ instructions() → 每次读取 compaction.md → provider request
```

行数来自当前工作树，包含空行和测试代码；没有生成代码。`server/src/run/engine.rs`、`server/src/run/compaction.rs` 的既有 checkpoint、重试和前缀逻辑没有移动。

## 改动文件

- `server/src/config.rs`：定义 `prompts/compaction.md` 路径；区分缺失/空白、有效内容和带路径的读取错误。
- `server/src/cursor/prompting/compiler.rs`：为 `Mode::Compaction` 增加可选路径，并保留其他模式的静态 Prompt。
- `server/src/app.rs`：启动时把受管数据目录下的压缩提示词路径传给编译器。
- `server/src/run/compaction.rs`：统一自动压缩的提示词读取和内置回退。
- `server/src/run/engine.rs`：自动压缩 provider 请求使用运行时提示词。

## 验证

本地使用新的 macOS SDK 和独立 Cargo 目标目录完成：

```text
cargo fmt --all -- --check                         通过
git diff --check                                    通过
cargo clippy --locked -p cursor-server --all-targets -- -D warnings
                                                     通过
cargo test --locked -p cursor-server --all-targets -- --nocapture
                                                     255 passed, 14 ignored, 0 failed
```

新增测试覆盖：

- 配置文件缺失、空白内容和修改后重载；
- 非法 UTF-8 错误包含具体路径；
- 手动压缩 Prompt 重载、模型名占位符和其他模式隔离；
- 自动压缩提示词重载及内置回退。

PR #17 的两套 GitHub Actions 运行均通过：

- [42098864508](https://github.com/haohaomin/cursor-byok/actions/runs/42098864508)
- [42098964651](https://github.com/haohaomin/cursor-byok/actions/runs/42098964651)

两次运行的 Frontend、Rust (Linux)、Desktop Rust (Windows)、Desktop Rust (macOS) 和 Sourcery 检查均为通过。PR 当前保持 OPEN，未自动合入。
