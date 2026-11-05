# 上游 PR #350：自定义上下文窗口优先

## 结论

已在当前 `origin/main` 上选择性重实现上游 [#350](https://github.com/leookun/cursor-byok/pull/350) 的核心行为，并准备独立个人 fork PR。没有合入上游旧模型目录、旧 protobuf 字段或与本仓库无关的刷新逻辑。

配置了模型上下文窗口后：

- Cursor 模型目录把配置值放在 Context 列表第一项；与内置档位相同的值复用内置标识和名称。
- 配置值成为默认模型变体，并且不再显示 `(Custom)` 后缀。
- 请求中的 Cursor 上下文选择不会覆盖已保存的模型上下文配置。
- 未配置自定义上下文时，继续保留 Cursor 选择的上下文值。
- 桌面模型编辑器说明明确提示自定义上下文的优先级，中文、英文和巴西葡萄牙语目录均已更新。

## 当前实现

```text
apps/desktop/src/features/models/CursorModelEditor.tsx
  └─ 说明配置后的上下文优先级
server/src/cursor/services/model_catalog.rs
  ├─ 统一上下文值归一化
  ├─ 生成 Context 选项和默认变体
  └─ 保留插件模型的默认 200K 行为
server/src/model/configuration.rs
  └─ 应用已保存的模型上下文配置
```

## 验证

在独立工作树 `/Users/renhao/.codex/worktrees/issue350-context-priority/cursor-byok` 执行：

- `cargo fmt --all -- --check`：通过。
- `git diff --check`：通过。
- `cargo clippy --locked -p cursor-server --all-targets -- -D warnings`：通过。
- 模型目录定向测试：6 项通过。
- 模型配置定向测试：2 项通过。
- `cargo test --locked -p cursor-server --all-targets -- --nocapture`：255 项单元测试通过，14 项忽略；所有集成测试二进制通过。
- `npm run i18n:scan`：通过。
- `npm run check`：通过，包含 TypeScript、Node 类型检查和生产构建。

尚未进行真实 Cursor 窗口和供应商请求验收；该限制不影响本地协议和配置逻辑测试。
