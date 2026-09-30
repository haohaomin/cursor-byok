# Cursor 工具卡片误显示 Skipped：独立修复

分支：`codex/fix-tool-card-skipped`，基于 main `3f1b202`；不包含 #421 的 Responses 文本解析修改。

## 问题与处理

Cursor 在收到文字或思考增量时会取消未完成的工具卡片。若模型在同一次响应中输出“工具调用 → 文字”，原服务端立即转发文字，而实际工具结果稍后才返回，卡片便保留 cancelled，即使命令成功仍显示 Skipped。

现在从首个工具调用开始暂存后续文字、思考及其结束事件。当前轮所有工具的结果事件发送完后，再按原顺序释放这些内容。保存到 Cursor checkpoint 的展示步骤使用同一顺序；模型历史、工具执行和审批保持原行为。

```text
server/
├── src/cursor/conversation/
│   ├── output.rs                # 1205 行：协调运行事件、工具结果与 checkpoint
│   └── output/narration.rs      # 102 行：暂存并按序发送文字/思考及记录展示步骤
└── tests/tool_presentation.rs   # 423 行：完整协议链路、重放与模型历史断言
```

以上为实测总行数，包含注释和空行；测试单独列出，无生成代码。没有模块移动、合并或删除。

```text
Provider → Run（原始历史与持久化保持不变）
             ↓ 运行事件
ConversationOutput → 工具卡片/执行请求 → Cursor 执行
             │                            ↓
             │                      工具结果提交
             │                            ↓
             └→ NarrationBuffer ← 所有工具完成事件已发出
                       ↓
                 后续文字/思考 → Cursor 实时展示
                       └──────→ StepBuffer → checkpoint 展示重放
```

- 普通文本在工具调用之前仍立即发送。
- 成功、失败、拒绝结果不被改写。
- 后台 Shell 的转后台确认即为本轮结果，不等待进程退出。
- 同轮工具结果可以逆序返回；数据库已全部提交也不代表客户端已收到所有完成事件，因此检查已处理的调用集合后才释放文字。
- 模型重试先关闭失败调用的卡片，再释放此前的展示内容；失败尝试不进入模型历史。
- 中断生成清除暂存内容；停止整个运行不会把暂存内容带入另一运行。

## 验证

先运行 5 个协议回归测试，修复前全部失败于“工具尚未完成时出现文字”，修复后全部通过。

最终 8 个专项用例通过：Shell 成功、失败、拒绝、转后台、同轮多工具、逆序并行结果、用户取消、模型重试。测试通过真实服务端 TransportRegistry、Run、SQLite 和 protobuf 流，模拟客户端执行返回；没有执行真实 shell 命令。

用例同时验证：完成事件先于后续文字/思考，文字不重复，最终 checkpoint 重放顺序一致，正常轮次的 provider 历史保持既有前缀，失败重试请求历史相同。

- `cargo test -p cursor-server --all-targets`：277 项通过（含前 7 项专项）。
- 补充逆序并行用例后，`cargo test -p cursor-server --test tool_presentation`：8/8 通过。
- `cargo clippy -p cursor-server --all-targets -- -D warnings`：通过。
- `cargo fmt --all -- --check`、`git diff --check`：通过。

构建沿用本机 MacOSX26.5 SDK 与 Rust 自带 ld64.lld，复用主仓库 target。测试日志位于 `/tmp/cursor-tool-presentation-{tests,focused,clippy}.log`。

## 当前状态

代码与协议回归验证完成；未替换已安装应用、修改用户配置、合入 main 或发布。真实 Cursor UI 尚未对修复版复测，历史已保存的 cancelled 卡片也未修改。
