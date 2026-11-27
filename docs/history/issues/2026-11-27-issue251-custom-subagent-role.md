# Issue #251：自定义子代理角色提示未传给模型

日期：2026-11-27。来源：[上游 #251](https://github.com/leookun/cursor-byok/issues/251)，当前仍为 OPEN；本轮读取正文、评论与关联时间线，未发现关联 PR。基线：`6bc343fcce153cbe78c940556b923bf8404b03bf`（v1.2.11 发布记录之后）。

## 结论与范围

独立复现了当前 Rust 服务端的漏传：请求携带 `subagent_type_name` 和 `RequestContext.custom_subagents[].prompt`，但 provider 只收到角色名称和简介，没有选中角色的正文。不同自定义角色因此无法仅凭该字段取得各自指令。

本轮修复角色正文投影。#251 还包含提示词编辑面板需求，未实现；真实 Cursor 调用角色、真实模型是否按角色工作尚未验收。原报告提及的旧版通用 explorer 系统提示不在当前实现中，不能将原报告全部症状等同于这次自动化复现。

## 已核对的路径

```text
AgentRunRequest.subagent_type_name          # 选中的角色名称
  + RequestContext.custom_subagents         # 内联或 blob 水合后的角色定义
  → compile/subagents.rs                    # 精确选择当前角色，编译名称、简介和 prompt
  → compile/run.rs                          # 合并角色指令与子代理模型信息
  → compile/break_messages.rs               # 按内容去重，变化时追加 request-context
  → canonical history / checkpoint / store # 保存事件身份，恢复时保持历史前缀
  → provider ModelRequest                   # 当前角色指令可见，其他角色正文不可见
```

根 Agent 仍仅看到角色目录。没有选中名称、名称不匹配、或正文为空时，不注入其他角色指令。角色更新追加新上下文；原系统提示、工具定义及已有消息不因角色更新而改写。自动压缩沿用已有机制，只保留最新上下文。

不改模型选择、工具权限、客户端安装文件，也未实现 `custom_system_prompt` 的全量系统提示替换。

## 客户端静态证据

本机 Cursor **3.31.15** 的打包 JavaScript 经只读检查：

- `cursor-local-agent-runtime/dist/main.js` 的 `Voe` 将自定义角色的名称、简介和 `prompt` 编译成角色提醒；`Xoe` 从自定义角色列表构造配置；`ZPe` 将角色提醒交给子代理提示生成流程。
- `Gnt` 对 `customSystemPrompt` 的处理是替换整个系统提示。它与角色定义中的 `prompt` 语义不同，本轮没有将两者混用。
- `cursor-agent-host/dist/main.js` 的只读文本检查可见子代理类型、角色定义和运行选项的传递；没有捕获真实界面请求，不能从静态代码推断当前窗口已走其中某条路径。

使用 REA CLI 读取 `package.json` 和以上两个 JS。完整分析证据 ID：`ev_a485fa596bb60a5bf97cc88b22f0197ca1c4331db3c49acfa96d3cc6ff31806b`；根目录身份 SHA-256：`19aca926ce48afeb989e4a007088564dacb1ad6e10eaf467435d2112b7270d81`。分析覆盖为 partial：Agent Host 文件触及 1024 MiB 分析内存上限；局部语义图触及节点、未知项和别名预算；其余 12156 个文件未作语义分析。上述具体函数结论来自原文件的定向文本阅读，不宣称 REA 建立了完整调用图或观察到实际执行。

| 文件（相对 Cursor app 根目录） | SHA-256 |
| --- | --- |
| `extensions/cursor-local-agent-runtime/dist/main.js` | `a637fafae17faca9c0d07b6fac0a31c1cb63915335a78527e004567142cd834c` |
| `extensions/cursor-agent-host/dist/main.js` | `c7170dacc639cfd42a671527fb40175594096f6db53c2a96fe738b85f7121528` |

## 失败复现与验证

使用隔离 SQLite、生产 Cursor HTTP/BidiAppend 路由与传输层、FakeProvider 捕获最终模型请求。没有调用真实供应商或读取正式对话。

基线新增两项测试均失败：选中 docs-writer 后模型请求缺少角色标记；A → B → A 只有一个目录上下文，预期三个角色上下文更新。

修复后的五项定向测试通过：

1. docs-writer/reviewer 只收到各自正文；根 Agent、未知名称、内置 explore 不收到自定义角色正文。
2. 相同角色不重复注入；A → B → A 保留三个不同事件；重试同一输入去重；每轮重建传输注册表并从检查点恢复，历史、系统提示和工具前缀不变。
3. `RequestContextPartReferences` + `PreFetchedBlob` 的角色定义可到达模型。
4. 清空角色正文追加新上下文，不取另一角色的正文，也不改写旧历史。
5. 模拟 provider 上下文超限，实际经过摘要和继续调用后，只保留最新角色上下文。

全工作区回归 **650 通过、0 失败、14 忽略**；`cargo clippy --locked --workspace --all-targets -- -D warnings`、格式、差异和文档链接检查通过。本机构建沿用 `profile.dev.package.sqlx-macros.strip="none"` 命令参数，未改项目构建配置。提交、合并和发布状态待后续补齐。FakeProvider 测试只证明传递、存储和恢复行为，不证明真实模型遵循角色，也不替代 Cursor UI 验收。
