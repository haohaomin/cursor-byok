# 上游 PR #229：当前 Rust 提示词正文注入防护

## 结论

个人 fork 按当前 Rust 架构重新实现了上游 PR #229 的安全意图，没有恢复已经退出的 Go `internal/backend/agent/prompt` 路径。未提交或记录任何账号、文件正文或令牌。

上游固定版本：[`9e1a27fe8d8de804bc7c4a48baefa3dde96117e7`](https://github.com/leookun/cursor-byok/commit/9e1a27fe8d8de804bc7c4a48baefa3dde96117e7)。

## 当前数据流

```text
Cursor RequestContext / UserMessage
    ↓
server/src/cursor/compile/context.rs
    ├── 规则、选中文件、终端、命令、Skill、外部链接正文
    └── 结构标签闭合序列中和
    ↓
server/src/cursor/compile/rules.rs
    └── user_rule 正文保持可读，只中和结构性闭合标签
    ↓
server/src/cursor/compile/break_messages.rs
    ├── 用户查询与中断上下文
    └── system context injection
    ↓
server/src/cursor/compile/insert_messages.rs / run.rs
    └── 后台通知、任务结果和已批准计划正文
    ↓
稳定的 Runtime Message / Request Context
```

## 实现边界

- 新增 `neutralize_prompt_body`，只中和结构性闭合标签的 `<`，例如 `</file>` 变为 `&lt;/file>`。
- 大小写和标签内空白均会被识别；普通 HTML、泛型、比较运算和移位运算保持原样。
- 规则、云端规则、选中文件、代码、终端、命令、Skill、外部链接、环境/仓库状态、用户查询、任务通知、任务结果和执行计划均在进入包装器前处理。
- 属性值继续使用现有 `xml` 转义；MCP 描述和 schema 继续使用原有完整 XML 转义。
- 保持 append-only 历史与已有请求上下文身份计算；没有重写历史消息或引入兼容分支。

## 验证

- `cargo test --locked -p cursor-server --lib`：255 通过，14 个忽略。
- `cargo test --locked -p cursor-server --all-targets`：255 个库测试通过，全部集成目标通过，14 个忽略。
- 定向回归：`prefix_stability` 13、`interrupt` 23、`conversation_delivery` 18，全部通过。
- `cargo clippy --locked -p cursor-server --all-targets -- -D warnings`：通过。
- `cargo fmt --all -- --check` 与 `git diff --check`：通过。
- 新增测试覆盖结构标签逃逸、大小写/空白边界、正常源代码保持不变、规则/文件/终端正文、已批准计划正文。

以上是服务端协议和提示词编译验证，不代表真实 Cursor UI 或真实模型账号验收。
