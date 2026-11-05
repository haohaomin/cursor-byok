# Issue #458：Windows 终端命令参数被二次解析

上游 [PR #458](https://github.com/leookun/cursor-byok/pull/458) 报告 Windows `open_terminal_with_command` 使用 `cmd /C start cmd /K` 时，`start` 会再次解析命令字符串，导致参数、引号或重定向内容错位。

## 采用范围

只采用终端启动修复：直接启动 `cmd /K`，并通过 Windows `raw_arg` 原样传递命令字符串。上游 PR 同时把 CA 证书从 Machine 存储改为 User 存储，这会改变已有安装的信任范围，本次不采用。

## 修改路径

```text
apps/desktop/src-tauri/src/desktop.rs
└── open_terminal_with_command  # Windows 直接使用 cmd /K + raw_arg
```

## 验证范围

- `cargo fmt --all -- --check`
- `git diff --check`
- 本机桌面 Rust 构建/测试
- Windows CI 的桌面 Clippy，确认 Windows 专用 `raw_arg` 路径可编译

尚未在真实 Windows Cursor 界面点击执行命令；这项边界不写成 UI 验收通过。
