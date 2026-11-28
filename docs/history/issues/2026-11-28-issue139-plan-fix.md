# Issue #139：编辑已有计划文件

状态：候选修复已实现，隔离协议验证通过；全量回归与合入检查进行中。真实 Cursor UI/模型尚未验收，不能标记原 Issue 全部解决。最新公开版本仍为 v1.2.17。

## 复现与行为变化

上游 [#139](https://github.com/leookun/cursor-byok/issues/139) 只有截图。当前源码基线 `c512727` 的四项协议测试中，Plan 缺 Write 广告、WriteArgs 缺 Plan 标记、非文档编辑未拦截三项失败；Agent 编辑源码对照通过。模型强行调用 StrReplace 时旧 dispatcher 本来就能读写，问题不等同于工具调用 unavailable。

候选实现允许 Plan 通过 Write/StrReplace 修改已有文档，保留原路径，并在 WriteArgs 字段 7 发送 `plan_mode=true`。CreatePlan 提示与成功结果明确每次调用创建新文件；修改已有计划须读取并编辑同一 URI，包括需要调整的 YAML todos。

新增恢复测试还复现：省略客户端 checkpoint 后，旧编译过程默认 Agent，从而失去 Plan 限制。修复从本地对话头读取模式；显式 Agent 消息和 ExecutePlan 批准仍优先，允许进入实现阶段。

## 执行与所有权

```text
AgentRunRequest / 本地已保存的 checkpoint
  → compile/run.rs：确定本轮模式和客户端环境
  → ExecContext.plan：随该次编辑保留 Plan 规则
  → tool_call_dispatch/edit.rs：读取前检查文档路径
      ├─ 不允许：本地 Edit 错误卡片 + 模型工具结果；无客户端执行
      └─ 允许：ReadArgs → 客户端 ReadResult
                   → codec/response.rs：写入前再次检查
                   → WriteArgs(path 原样, plan_mode=true)
                   → 客户端执行/审批 → Edit 结果 → 历史/checkpoint
```

模式属于会话/当前 Run；恢复创建的新 Run 从对应 checkpoint 取得模式。显式用户模式和批准操作可以切换；同一编辑读写阶段共用其执行上下文。服务端限制编辑类型，客户端继续拥有文件解释、实际权限和审批。

```text
server/
├── prompt/cursor/modes/plan.json       # Plan 工具目录，尾部追加编辑工具
├── prompt/cursor/plan/runtime.md       # 规划与实施的行为边界
├── prompt/cursor/tools.json            # CreatePlan 的新文件语义
├── src/cursor/compile/run.rs           # 模式恢复和执行上下文
├── src/cursor/tools/edit/plan.rs       # 文档类型及客户端路径校验
├── src/cursor/tools/edit/plan/         # 路径测试和原生对照固定语料
├── src/cursor/tools/codec/             # Read/Write 及 Plan 字段编解码
├── src/cursor/tools/tool_call_result/  # Edit 错误与 CreatePlan 结果
└── tests/plan_editing.rs               # 创建、修订、拒绝、恢复、批准协议往返
protocols/cursor/agent_v1.proto         # WriteArgs 字段 7
```

## 路径契约与边界

与固定 Cursor 3.31.15 的静态契约及纯函数探针对照，允许 `.md/.markdown/.mdown/.mkd/.mkdn/.mdx`，不限计划目录；另允许 `.cursor/projects/<单段>/canvases/<单段>.canvas.tsx`。路径先按客户端平台解析点段，再检查后缀和 Canvas 位置；Unicode trim 与 ECMAScript 一致。

使用 `typed-path` 进行跨平台词法路径处理，使用已有依赖链中的 `percent-encoding` 解码 URI。不会读取服务器 cwd、HOME、文件系统 realpath 或猜测客户端 home；相对路径要求客户端提供 process_working_directory，`~/` 仅在客户端明确标记工作目录就是 home 时展开，否则返回可操作的绝对路径错误。当前工具 schema 本身要求绝对路径。检查用的规范化路径不替换发给客户端的原始 path。

原生隔离探针 141 个输入 × 两个平台，共 282 行。其中 30 行是原生 file URL 转换报错后回退成普通字符串的路径；本实现明确拒绝此类无效 URI，不复制这一回退。其余 **252 个有效路径/URI** 作为固定语料全部匹配原生允许/拒绝结果。合成 cwd/home 仅用于测试，不代表真实客户端环境。

该规则不是通用沙箱：未新增 Shell/Delete/MCP 的权限系统，未确认符号链接、文件权限或实际写审批结果。允许一个文档类型也不等于文件存在或客户端会批准。

## 验证

- 基线 3 失败/1 通过；最初候选四项均通过。
- 恢复模式追加测试先失败（缺客户端 checkpoint 时限制消失），修复后通过。
- 10 项协议用例：工具广告、同路径修订、Plan 标记、源码拒绝、Write/EditNotebook 原生错误卡片、Windows URI、创建后按返回 URI 修订、恢复、批准、远程相对 Canvas。
- 8 项路径单测，其中包含 252 个原生对照输入；字段 7 另由独立 prost 结构解码验证。
- 前缀测试中 Plan 工具名单及 schema digest 因明确新增工具和修正说明而更新；旧消息不重写。
- 全工作区、Clippy、最终 CI 和发布结果将在完成后补记。[结构化证据](2026-11-28-issue139-plan-evidence.json)。

真实验收仍需：在隔离 Cursor 会话 CreatePlan，读取返回 URI，明确修改该文件，核对文件身份不变和实际内容变化，再验证审批及批准后的代码编辑。本轮未操作正式 Cursor 配置、未执行 20 分钟测试。
