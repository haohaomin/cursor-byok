# Issue #139：当前 Cursor CreatePlan 静态契约

当前候选实现与验证进展见[修复记录](2026-11-28-issue139-plan-fix.md)；以下保留最初静态调查和基线结果。

结论：Cursor 3.31.15 的完整 Plan 工具集合支持直接编辑已知计划文件。现代 CreatePlan 的不同调用会创建新的计划 URI；“省略 name 就覆盖当前计划”不是当前消费者实现的语义。

## 计划身份与执行路径

```text
首次 CreatePlan(args, toolCallId)
  → runtime listener.query(createPlanRequestQuery)
  → IDE 当前 toolCallId 对应 bubble.additionalData.planUri
       ├─ 已存在：复用当前调用流式生成的 URI
       └─ 不存在：Yoa(toolCallId) → createPlanFile → createPlanForComposer
                    → 随机 8 位后缀 → ~/.cursor/plans/<name>_<id>.plan.md
                    → 无权限时尝试 workspace/.cursor/plans
  → CreatePlanResult.planUri 返回模型

修改已知计划
  → 模型 Read(path) → StrReplace/Write(path)，GPT5 工具变体也可 ApplyPatch
  → runtime 按 PLAN 模式检查允许的路径类型
  → Exec Read / WriteArgs(path, fileText, toolCallId, planMode=true)
  → 客户端执行器、实际文件权限和写入审批
```

同一次调用的 partial/completed/query 共用 toolCallId，所以会更新同一个刚建立的计划。新调用不使用会话最新计划 URI；省略 name 时消费者从正文取标题，然后仍分配新 ID。现包虽然保留 getOrCreatePlanFile 定义，但已查现代消费者没有调用它；三份固定文件中该名称仅命中此定义。

关键偏移（Unicode 字符，不是字节）：
- runtime R3570244 CreatePlan factory；R3572492 更新说明；R3572406 参数；R3584180 结果说明。
- workbench W16907134 同调用身份缓存；W16916061 query consumer；W15580974 createPlanFile；W15549778 真正分配新 ID；W15548825 文件目录与权限降级。
- backend artifactsFolder 路径 R3571144 同样用新随机后缀写入 plans/，不是覆盖最近计划。

## 工具可用性与权限边界

完整默认 generator R3962850 / R3974513 保留编辑工具；Anthropic/Google 等为 StrReplace+Write，GPT5 分支用 ApplyPatch。Composer generator R3993346 / R3999203 只在 ASK 过滤和 Graphite 等条件去掉编辑，PLAN 本身不触发该去除。仍受 allowedTools/excludedTools、最小 harness、自定义工具集及其他配置影响，不能据此声称每个实际窗口都会显示这些工具。

原生 uBe（R3544115）在 PLAN 时检查 kLe（R3526152）。先经 tie（R2452779）处理 file://、前导 ~ 与 node:path.resolve，再允许：

- trim 后转小写的路径以 .md、.markdown、.mdown、.mkd、.mkdn、.mdx 结尾；不限 .plan.md，也不限计划目录。
- trim 后把反斜杠改为正斜杠，匹配大小写不敏感的 /.cursor/projects/<单段>/canvases/<单段>.canvas.tsx。

node:path 为宿主平台实现（R407702）；Windows/POSIX 路径解析随宿主平台，Canvas 正则另显式兼容两种分隔符。这个纯路径守卫不做 realpath、文件所有权或计划注册表查验；后续执行权限仍由客户端处理。不能把该守卫理解成任意文件可写。

$Be（R3552079）在编辑读取前检查，KBe（R3556372）在实际写入前再次检查并发送 planMode。WriteArgs field 7 是非 optional 的 proto3 bool，默认 false：runtime R5049868、host H3204728、workbench W12013861 三处独立声明一致。WorkBench 写入转发器 W16439177 转给 remoteWriteExecutor；客户端写审批服务 W16768594 处理工具上下文、工作区外保护、配置保护等。本次未定位三份文件以外执行端如何消费 field7，不能假定仅发字段就能代替 runtime 守卫。

## 当前仓库可独立补的缺口

1. Plan 工具目录没有 Write/StrReplace/ApplyPatch，无法执行原生建议的直接计划文件编辑。
2. CreatePlan 工具描述的“传完整新版并省略 name”与当前消费者新建 URI 行为冲突；只修文案不能补出编辑能力。
3. Plan runtime 提示没有允许编辑计划文档的边界。
4. WriteArgs 声明与序列化缺 bool field7；若开放编辑工具，还须同时补模式感知的路径守卫，不能仅增加工具名。

建议下一轮有限复现：在服务端 transport fixture 中让 Plan 模型调用现有文件的 StrReplace，核验工具广告缺失、Plan 标记和路径守卫；修复后验证同一 path 的 Read/Write 与 plan_mode=true，并验证非 Markdown/非允许 Canvas 在发 Exec 前拒绝。对实际 IDE，再做首次 CreatePlan → 读取返回 URI → 明确修改该 URI → 文件身份不变的短验收。新的 CreatePlan 调用生成另一 URI 应单独作为消费者对照，不作为原地修改实现。

## 证据边界

本报告基于已固定 SHA 的 Cursor 3.31.15 三份打包 JS，哈希与字符偏移详见 evidence.json。复用 REA inventory ev_a485fa596bb60a5bf97cc88b22f0197ca1c4331db3c49acfa96d3cc6ff31806b；原 inventory 有 Agent Host 解析 OOM（1024 MiB）、节点/别名/未知边界及 12156 未选目标，覆盖不完整。

最初静态调查未运行 Cargo、真实客户端、UI、账户或网络，也未修改仓库；后续基线与候选修复的 Cargo 验证另记在下文及修复记录。原 issue 只有截图，没有复现步骤。本报告证明当前静态契约及本地代码缺口，不能声称已复现截图现场或完成真实文件修改验收。暂不扩展既有计划附件/上下文如何注入的完整调用图。

## 服务端基线复现（2026-11-28）

基线 `c512727`：4 项隔离测试中 3 失败、1 通过。缺失 Write 广告、WriteArgs 丢失字段 7、Plan 可越过文档类型限制均已确认；普通 Agent 编辑源码对照通过。模型强行调用 StrReplace 时当前 dispatcher 已能读写，因此“工具没有广告”不能写成“调用返回 unavailable”。该基线复现阶段只新增测试与证据，尚未修改生产代码。日志哈希见[证据](2026-11-28-issue139-plan-evidence.json)。

路径补充：141 个固定输入分别按 POSIX/Windows 运行，共 282 行隔离纯函数结果。覆盖点段、Canvas 边界、扩展名、Unicode 空白、URI、相对路径与 `~`。全部 cwd/home 都是合成值；守卫 `allowed=true` 不等于实际文件有效或最终获得写权限。已查 StrReplace 外层未额外规范化，Write 外层只做特定路径拼写纠正；不把守卫内部 normalized_path 当作最终客户端写入目标，也不使用服务端 cwd/HOME 代替客户端路径。哈希和有限入口追踪见证据补充。
