# Issue #244：项目根对话的用户消息

日期：2026-11-28；复现基线 `46affbc`（PR #68）。PR #69 已合入 main（`c512727`）；最终提交 `78d3d5e` 的分支、PR 与主线 CI 全部通过，Codex 审查无发现。v1.2.17 已发布，20 项资产、7 份签名和三个更新清单实际下载核验通过；真实 Cursor 项目界面未验收。

项目根对话缺少 `SendMessage` 已独立复现：合成项目请求没有此工具，模型调用时生成兼容错误卡片；两项用例失败，普通聊天对照通过。现已接通根项目身份、文本校验、原生卡片及消息步骤索引，并修复服务重启后丢失项目历史呈现的问题。

## 当前实现

```text
server/
├─ prompt/cursor/tools.json          # SendMessage 文本参数和使用说明
├─ src/cursor/projects.rs            # 根项目身份与用户输出指引
├─ src/cursor/compile/run.rs         # 按身份追加工具和当前轮上下文
├─ src/cursor/tools/send_message/
│  ├─ mod.rs                        # 文本校验、成功/失败原生卡片
│  ├─ links.rs                      # 代码区外短代理链接的格式校验
│  └─ tests.rs                      # 参数、JS 文本语义、卡片回归
├─ src/cursor/checkpoint/
│  ├─ builder.rs                    # 保存根身份标记
│  ├─ turns.rs                      # 保存 SendMessage 步骤索引
│  └─ derived.rs                    # 无客户端检查点时恢复本地呈现
└─ tests/project_messages.rs        # 协议、历史、重启、压缩、续跑验收

UserMessage.projectDetails / 检查点根标记 / 最近历史边界
  → 根身份判定（子代理执行排除）
  → 原有工具末尾追加 SendMessage + 当前轮输出指引
  → 模型工具调用 → 文本和链接校验
  → 一份 SendMessageToolCall 卡片 + 一份模型工具结果
  → SQLite 检查点保存根身份、原生步骤与消息索引
  → 同一工具轮继续模型；后续 Run 从检查点恢复
```

该工具不产生 Exec/Interaction 执行请求，不把正文复制为普通 TextDelta。通知启动的 Run 仍按既有协议请求当前工作区上下文；这与 SendMessage 的本地呈现是两个环节。普通助手文本保持原始历史，不拦截或删除模型输出。

根身份依据当前 `projectDetails`、持久化根标记和最近历史边界恢复，不使用旧 mode 6。根项目的 Multitask 规范为 Agent；普通聊天、首次子项目、侧边对话、best-of-N 和子代理执行不会误启用。Cursor 原生 UI 禁止把根项目重新挂为子线程，因此不会根据后续缺省 metadata 清除已保存的根身份。历史恢复遇到普通非模拟用户消息即停止，不从任意旧项目消息推断当前身份。

校验使用 JavaScript trim/UTF-16 语义，裁剪后必须为 1–100000 个 UTF-16 单元。代码区外 Markdown 的 `bc-` 短链接按本机原生默认契约校验；不查询目标是否存在或有权限。普通 URL 原样保留。未知参数明确失败，错误仍使用原生 SendMessage 卡片。

## 验证

- 基线：两项项目工具/卡片用例失败，普通聊天对照通过。
- 重启回归：先复现消息步骤丢失，再恢复项目检查点的 turns/索引；通过 SQLite 关闭后重开验证。
- 8 项集成测试通过：原生卡片、非根角色、重启、最近历史边界、显式 Resume、后台完成通知、有/无客户端检查点、手动压缩；原消息索引保留，新增消息不覆盖旧消息。
- 3 项单测通过，覆盖字符串、空文本、UTF-16 上限、未知参数、根身份限制、原生错误卡片 protobuf 往返以及链接/代码区规则。
- 限定原生纯函数对照先发现分隔符回溯与 Unicode 空白两类差异；修正后 2191 个固定输入零差异，另 1099 个链接/文件片段输入零差异。
- 同一 Run 与无客户端状态的续跑保持投影历史前缀；客户端检查点导入会按既有规则生成 cursor-root 消息 ID，此边界比较内容/角色一致，导入后的模型轮次再比较完整前缀。

源码 `58c62d0`：完整工作区 **718 通过、0 失败、14 忽略**；Clippy `--workspace --all-targets -- -D warnings`、13 项历史前缀测试及格式检查通过。接上主线 v1.2.16 只引入五个版本文件和已核验的 Await 发布记录，server 文件树与完整回归时一致。PR #69 已合入 main（`c512727`）；最终提交 `78d3d5e` 的分支、PR 与主线 CI 全部通过，Codex 审查无发现。v1.2.17 已发布，20 项资产、7 份签名和三个更新清单实际下载核验通过；[资产证据](../2026-11-28-v1.2.17-release-evidence.json)。Sourcery 因配额跳过，不记为通过。测试使用 FakeProvider、合成 protobuf、临时 SQLite 或隔离纯函数；未修改正式代理、数据库或 Cursor 安装。

## 未完成范围

这只是项目根对话的文本输出通路。SendToAgent、云 worker 投递、线程回复、消息替换、附件、Agent Store、完整协调器提示与真实云端授权未实现或未验收。静态证据不能证明真实项目根请求会路由到 BYOK；不能据此宣称完整 Projects 已支持或 #244 已解决。#155 仍保持部分完成，未测 20 分钟任务。

来源哈希、字符偏移、反例与测试摘要见[结构化证据](2026-11-28-issue244-project-messages-evidence.json)。
