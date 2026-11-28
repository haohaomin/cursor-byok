# Issue #244：现代 Await 的实现契约

日期：2026-11-28；仓库基线 `17afcdf`。本轮只读核对已安装 Cursor 3.31.15 的本地 runtime、Agent Host 和 workbench；三个文件哈希再次匹配。复用 REA `ev_a485fa596bb60a5bf97cc88b22f0197ca1c4331db3c49acfa96d3cc6ff31806b` 的部分覆盖身份记录，未重新声称完整调用图，未运行客户端。来源偏移及摘要见[静态证据](2026-11-28-issue244-await-evidence.json)。

## 结论

现代 Await 是统一等待工具，具有 Shell、子代理、无 ID 纯等待三个分支。基线仓库只有 Proto 声明：`SubagentAwaitArgs/Result` 已接入 Exec oneof 37，`AwaitToolCall` 已接入工具卡片 oneof 42；工具目录、派发、结果转换没有可用的 Await 通路。不能只实现子代理分支后宣称整个工具完成，也不能直接恢复旧 PR #245 的等待别名。

```text
基线仓库
protocols/cursor/agent_v1.proto             # 已有等待消息和卡片声明
server/prompt/cursor/tools.json             # 未提供 Await 工具
server/src/cursor/tools/tool_call_dispatch/ # 未派发 Await
server/src/cursor/tools/codec/              # 未构造等待请求和卡片
server/src/cursor/tools/tool_call_result/   # 未处理 SubagentAwaitResult

待实现的统一执行路径
模型 Await(task_id?, block_until_ms?, pattern?)
  ├─ 无 ID → 可被新消息提前释放的等待
  ├─ 数字 ID → 读取 Shell 输出文件 → 解析完成 footer 或匹配 regex
  └─ 非数字 ID → 子代理状态等待 → 专用 Exec field 37
       → 已完成 / 仍运行 / 不存在 / 错误
       → AwaitToolCall + 模型结果 → 原有工具历史持久化
```

## 启用与参数

- 本地 runtime `FUe`（字符偏移 3486790）标识为 AWAIT；启用子代理且非 training-shell-only 时名为 Await，常规 Shell 专用模式叫 AwaitShell。名称本身不能证明能力。
- 生成路径 3973236 要求 longRunningJobs、已有 Shell 或 Task，另受 prompt profile 控制；4005296 路径由 enableAwaitTool 控制。子代理能力来自 enableSubagentAwaiting/enableAwaitForSubagents，现代 Exec 另需 enableBoundedSubagentAwait。
- AgentRunRequest 没有这些能力字段。BYOK 固定 `long_running_jobs` 不代表子代理 Await 或 bounded 已启用；安装包默认后二者 false，也不是当前窗口的运行值证据。
- 默认等待 30000 ms；配置可改默认值或要求显式 timeout。schema 上限为 `min(perCallLimitMs ?? 7140000, 7140000)`，执行另受 maxBlockUntilMs 约束。负数/非有限值回退默认，非负小数取整。
- ID 省略、空白或 `none`：等待大于零时纯等待，零等待报参数错误。
- 子代理不接受非空 pattern。启用完成通知时，原生提示要求对子代理只做零等待状态检查并依赖自动通知；执行器本身仍支持正时间有界等待。

## 子代理执行与呈现

```text
ExecServerMessage.subagent_await_args = 37
  agent_id: string; timeout_ms: uint32
ExecClientMessage.subagent_await_result = 37
  complete: agent_id, transcript_path?, tool_call_count, final_message?
  still_running: agent_id, transcript_path?
  not_found: agent_id
  error: agent_id?, error
```

Workbench 资源注册 16482660 → aKp 16456371 → awaitBackgroundSubagentBounded 16858690。它观察已有 `_backgroundPromises`：已有结果立即返回，零等待返回 stillRunning，否则注册等待者和计时器；超时或取消只清理等待者，不创建、恢复或终止子任务。

Host `ol`（1632917）观察后台运行；未追踪任务可能读取会话结束状态，因此零 timeout 也可能产生至少一秒的加载预算，不能把参数当作严格端到端耗时保证。

bounded=false 的旧路径（runtime 3494229）发送 `SubagentArgs`：resumeAgentId 为任务 ID，空 prompt、readonly=true、runInBackground=false，加当前父会话。Workbench 在 `long_running_jobs` 生效且参数组合匹配时才识别为等待；它不是现代 field 37。后续实现不应在缺少能力证据时自动切换到此路径。

返回转换（runtime 3495650–3501969）使用 `AwaitResult.success.complete/still_running`，挂到 AwaitToolCall。模型实际取得状态、输出路径、输出长度，Shell 完成另有退出码；`final_message` 和 `tool_call_count` 不进入公共 Await 返回值，完整内容需要后续读取 transcript。源码未解释舍弃字段的设计动机。

not_found 还可能读取 transcript 尾部 `turn_ended` 恢复完成状态，这是额外恢复行为。Workbench 30780756 根据 complete 卡片抑制后续通知，Host 1719990 对应处理只覆盖数字 Shell ID；不能假设两个客户端对子代理重复通知处理一致。

## 后续验收要求

1. 先复现工具不可用，并同时覆盖三种输入分支；不能以已有 Proto 代替执行成功。
2. 明确本维护版本的工具启用政策与当前客户端能力边界，不猜测运行时开关。
3. 验证现有任务的零等待、完成、有界超时、不存在和错误；取消等待不能取消子任务。
4. 验证状态卡片、输出读取、历史前缀、重试、取消和通知去重；区分同一工具调用内等待与后台完成通知创建后续 Run。
5. 真实 Cursor Exec 往返、卡片和云子代理仍需实机验收。本页为实现前审计；后续候选修改见[修复记录](2026-11-28-issue244-await-fix.md)，不构成真实 Cursor 验收完成；不进行 20 分钟任务测试。

## Shell 与纯等待补充核对

数字 Shell 使用 Exec ReadArgs/ReadResult（双方 field 7），每轮读 `<terminalsFolder>/<原数字ID>.txt`。完成取决于终端文件末尾的 `exit_code:` footer；没有 PID 或进程存活查询。先检查完成，再检查正文 regex，随后检查超时，每次轮询间隔 250 ms。输出长度优先采用正数 file_size，否则使用字符串的 UTF-16 长度。原生读取没有检查 truncated；本候选对截断/范围读取明确报错，防止据不完整文件误判。

无 ID 等待在原生实现中监听计时器、取消和已接纳用户消息。用户消息提前释放返回成功并带 `wake_reason=context_injection`；取消则报 Aborted。本维护版本现有新消息处理会中断并替换父 Run，候选沿用该生命周期，取消等待者并保留后台任务，不伪造原生同一生命周期内的成功唤醒。

子代理状态返回后，原生额外读 transcript 以取得输出长度；读取失败仍保留 complete/stillRunning 并使用长度 0。候选使用客户端给出的路径，不复制旧执行兜底或多路径/LS 搜索。完整静态来源偏移见结构化证据。
