# Issue #327：目标模式缺少 CreateGoal / UpdateGoal

日期：2026-11-27。来源：[上游 #327](https://github.com/leookun/cursor-byok/issues/327)，当前 OPEN；已读取正文、评论和关联时间线，未发现关联 PR。基线：`fe402da39e6d5f3bc156336c6b29d106b67967c8`。工作分支：`codex/goal-mode`。

## 结论与范围

当前服务端确实没有目标工具和目标续跑处理。已有 protobuf 定义不能替代工具目录、执行、状态保存及续跑分派。新增失败测试在基线中无法取得 CreateGoal 后的 `goal_state`。

本轮实现目标创建、active/complete 更新、持久化恢复、客户端自动续跑和连续三次无工具调用后的暂停。13 项目标协议测试和子代理续跑保留测试已通过，全工作区最终为 664 通过、0 失败、14 忽略；合入与发布状态见后续记录；真实 Cursor 的自动发起、原生目标卡片及真实供应商尚未验收。FakeProvider 和合成 GoalContinuationAction 不能证明桌面会自动发送通知。

## 分工与路径

```text
server/
├── prompt/cursor/                    # 根 Agent 模式的工具声明；子代理不开放目标工具
├── src/cursor/goals/
│   ├── action.rs                     # 校验目标归属、拒绝旧通知、生成一轮续跑输入
│   ├── admission.rs                 # 持有每个会话的自动续跑预约，拒绝并发重复通知
│   ├── tools.rs                      # 创建/更新目标，返回原生卡片和可持久化结果
│   └── mod.rs                        # 从已提交结果投影目标状态，累计活动时长
├── src/cursor/checkpoint/            # 将目标与既有会话检查点一起保存、恢复和压缩
├── src/cursor/conversation/          # 新 Run 接收目标动作；工具回合传递当前目标
├── src/cursor/tools/                 # 工具分派、原生 protobuf 卡片与执行身份
└── tests/tool_presentation/goals.rs  # 生产传输层 + 隔离 SQLite + FakeProvider 验证
```

```text
用户明确要求创建目标
  → 模型调用 CreateGoal
  → 工具结果提交 → checkpoint.goal_state = ACTIVE
  → 本轮结束
  → Cursor 空闲协调器发送新的 GoalContinuationAction
  → 校验归属、状态、计数和待完成工具
  → 追加稳定身份的模拟用户消息 → 新模型 Run
  → UpdateGoal complete → COMPLETE，后续通知只确认、不调用模型
```

同一个 Run 内的工具回合由现有运行时继续；跨轮自动续跑由 Cursor 发起。服务端没有增加独立的无限循环。三个连续续跑没有工具调用时，下次通知将目标置为 PAUSED；有工具调用会清零空转计数。普通用户消息可恢复暂停目标；自动通知不能自行恢复。用户在检查点中提交的暂停/清除状态被持久化；支持工具栏恢复为 ACTIVE，以及清除时直接移除 goal_state。已完成目标及已测试的旧计数/旧累计时长通知不会重新启动目标。自动续跑持有会话预约，并在激活前再次检查用户 Run，避免重复调用或取消用户任务。

目标工具的结果在 canonical commit 之后才进入检查点；重放旧历史不会覆盖客户端暂停。目标 ID 纳入模型回合身份，避免工具 ID 在不同回合重复时误认为同一个目标。工具目录、系统提示和旧 provider 消息不因目标变化被改写。检查点恢复会重建内部 assistant/tool 消息 ID；测试比较 provider 实际使用的 role/content，并单独比较 PromptSpec，而非将这些内部 ID 当成 provider 参数。

## 客户端静态依据

只读核对本机 Cursor **3.31.15**：`cursor-local-agent-runtime/dist/main.js` 中 `Sze` 构造 CreateGoal/UpdateGoal；更新工具只接受 active/complete，暂停由用户或空转保护控制。`XAe` 将目标续跑转换为模拟用户消息，更新续跑计数，达到三次无工具调用时暂停。`Ive`/`Ave` 校验 ACTIVE、UUID 和 conversation/agent session 归属；普通用户消息可通过 `Mve` 恢复暂停。

`workbench.desktop.main.js` 中 `_getGoalReconciliationSnapshot` / `_runGoalContinuationAction` 检查空闲、审批和待完成工具，并由客户端发起新动作；使用 `agent_goal_continuation` 开关。`resumeGoal` 把 PAUSED 改为 ACTIVE、清零空转计数并启动计时；`clearGoal` 直接移除可选目标字段。服务端 bootstrap 为该已实现路径固定开启此项，同时保留原有 BYOK RunSSE 路由配置。

Workbench SHA-256：`79a1c7cc4fc3b14df093b18829ab88c1ee955e60ea49bb7984dbaf488f033713`。局部运行时 SHA-256：`a637fafae17faca9c0d07b6fac0a31c1cb63915335a78527e004567142cd834c`。复用上一轮 REA 证据 `ev_a485fa596bb60a5bf97cc88b22f0197ca1c4331db3c49acfa96d3cc6ff31806b` 的文件身份，再做定向文本阅读。REA 分析覆盖为 partial；workbench 只做定向文本阅读。这些是静态依据，不代表当前窗口执行路径或真实自动续跑已经验收。未修改 Cursor 安装文件。

## 验证记录

- 基线两项失败：CreateGoal 后没有 goal_state，创建/完成和空转暂停链路均不可用。
- 初版发现用户暂停未持久化：旧 ACTIVE 通知会再启动；新增断言失败后已修复。
- 边界测试另行复现：待完成工具时错误启动续跑；不同模型回合复用工具 ID 时无法新建目标。已补充对应处理，正在跑完整回归。
- 定向场景：创建→续跑→完成、过期通知去重、三次空转暂停、普通工具清零空转、非法更新不改变目标、用户暂停/清除与会话归属、手动/自动压缩、缺省检查点恢复、待完成工具、跨回合工具 ID 重复。
- 第一轮完整回归曾为 659 通过、0 失败、14 忽略；Clippy 通过。后续审查新增并发和工具栏场景，不能用此前结果代替最终提交验证。
- 三项新增失败复现：并发通知争抢会话、通知取消活动用户 Run、工具栏恢复被旧状态判断拒绝。已加会话预约和激活检查，并按客户端语义处理恢复/移除目标。
- 最终定向测试 **13 通过**：另包括延迟通知保留后续用户工作/压缩摘要、同时通知只调用一次模型、活动用户 Run 不被取消、工具栏恢复/移除。
- 独立子代理回归 **1 通过**：已创建目标经历用户插入、子代理脱离和父任务完成后续跑，目标仍保留；此项在首次补测即通过，没有为它改写现有后台机制。
- 最终全工作区 **664 通过、0 失败、14 忽略**；Clippy、格式、差异及 Markdown 本地链接检查通过。命令使用 `profile.dev.package.sqlx-macros.strip="none"`，未修改工程构建配置。跨平台 CI 与发布状态待核验后补充。当前不标记为完整解决。

## PR 审查补充

[本 fork PR #65](https://github.com/haohaomin/cursor-byok/pull/65)，初版 head `dd9427bd4f69f187985b3f30400c1d7e62be9ab3`；初版 [PR CI](https://github.com/haohaomin/cursor-byok/actions/runs/45996692869) 和 [分支 CI](https://github.com/haohaomin/cursor-byok/actions/runs/45996579379) 四组检查均成功。

Codex 审查指出：通知携带旧检查点时，虽然历史已切到最新，但局部 goal 仍来自旧请求，可能撤销后续工具调用对空转计数的清零。新增测试在初版得到 idle=3（预期 1），已改为采用最新目标状态的运行计数，并单独保留客户端暂停/恢复及当前检查点上的目标编辑。定向测试现为 **13 通过**；补丁版工作区 **664 通过、0 失败、14 忽略**，Clippy 和格式检查通过；等待最新提交的跨平台 CI。

Sourcery 因每周 250,000 字符额度用完跳过，未记为审查通过。Codex 的上述审查针对初版提交；不将其描述为补丁版的独立复审。未发送再次触发机器人的评论。
