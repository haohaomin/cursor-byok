# PR #61：摘要输出上限与截断重试

来源：[本 fork PR #61](https://github.com/haohaomin/cursor-byok/pull/61)，作者 ShowBoy7816；来源 head `f41c4966ff71215a6ebe502412504f32c160b99c`。基线 `34ba1cc511cc7198ea104ee8b30da30c51397528`，工作分支 `codex/pr61-compaction-output-cap`。采用原始提交并在真实合并中补全，不重复 cherry-pick；来源作者与提交保留。

当前状态：已补全候选实现，本地验证通过，待跨平台 CI 与合并；未发布。v1.2.9 不包含本功能。

## 解决的行为

自动压缩此前固定给摘要 4096 个输出 token。摘要或供应商隐藏思考耗尽额度时，摘要失败且原历史保留，下一轮可能再次遇到相同问题。现在可在设置页设为 1024–65536，默认 16384；压缩调用还会取配置的模型输出上限与可用上下文约束后的较小值。因此设置 16384 不保证每次请求都使用 16384。

输出截断或供应商拒绝上下文过大时，缩小实际历史片段重试。每批最多三次此类重试，成功一批后恢复该批次的重试额度；缩小后的输入限额用于后续批次。摘要只在所有片段成功、重建上下文验证通过后替换历史；拒绝、超时、取消、空摘要或重试耗尽时，不把部分摘要写入正式历史。普通模型回复和手动压缩的重试规则不在本项范围内。

```text
apps/desktop/src/features/settings/CompactionSettingsCard.tsx  # 两项数值与保存期间禁用
apps/desktop/src/features/settings/SettingsPage.tsx            # 校验，只提交用户修改的字段
                     ↓ GET / PATCH /settings/compaction
server/src/control/                                          # 本地管理接口
                     ↓
server/src/store/settings/compaction.rs                       # 独立设置行、事务、读写范围约束
                     ↓ 每次自动压缩开始时读取一次
server/src/run/engine/compaction.rs                           # 单次压缩生命周期、每批最多三次重试
                     ↓
server/src/run/compaction/batches.rs                          # 提示/摘要/输出预留，按顺序切分历史
                     ↓ provider 返回 Stop 且完整
server/src/run/engine/compaction.rs                           # 重建检查点；提交后继续当前 Run
```

超时继续保存在 `run_compaction` 行；输出上限独立存为 `run_compaction_output_tokens` 的 JSON 整数。两项都修改时在一个事务中写入，GET 用一条查询取得一致快照。没有新增迁移、旧格式回填或兼容读取路径。管理 API 使用 PATCH；旧 PUT 路径移除。当前运行中的压缩沿用启动时读到的设置，下次自动压缩读取新值。

## 原补丁审查意见的处理

| 问题 | 本次处理与证据 |
| --- | --- |
| 用设置值覆盖模型自身输出上限 | 新增 1K 模型集成测试，原补丁实际发送 16384，测试失败；修复后为 1024。发现配置值原先仅在 provider 路由可见，现提前带入 ModelSpec 供压缩预算使用 |
| 大输出上限挤掉提示和历史输入 | 先计入提示与已有摘要，将剩余额度的一半作为输出上界；每次调用重新计算，保留输入空间 |
| 默认 32K 窗口 / 16K 输出时无法进入重试 | 删除 `window > output * 2` 门槛；未配置窗口和显式 32K 场景均验证可重试 |
| 直接减半总窗口导致下一批无输入预算 | 减半被拒绝请求的实际 transcript token 数，保留独立的提示/已有摘要/输出预留；单测断言重试片段确实更小 |
| 两个客户端保存整对象互相覆盖 | PATCH 只发送用户编辑的字段；两个设置各自持有一行，事务写入与并发存储测试通过 |
| 保存期间仍能修改/重复 Enter | 两个输入、保存、取消均禁用；挂起请求验证只产生一次 PATCH |
| 新字段的 serde default 回填旧结构 | 输出上限使用独立设置行，超时行结构不变；公开完整设置对象不接受缺字段，PATCH 缺字段表示不修改而非回填 |

截断判断读取 `ModelCycleFailure.finish_reason`，不依赖错误消息文字。普通 provider 错误不会因碰巧包含同一句文字而被当作输出截断。

## 验证

- 定向库、压缩、管理 API 与设置测试通过；包含完整原始答案在成功重试批次中逐字覆盖且不重复、三次重试耗尽保留原历史、先成功一批后失败仍不提交部分摘要。
- 预算单测覆盖 4K、8K、32K、100K 窗口，输出设置 1K、16K、64K；提示、之前摘要与当前批次总估计加输出预留不超过窗口的 90% 预算。
- 设置测试覆盖默认值、上下界、直接写入数据库后的读取归一、并发修改不同字段以及第二行写入失败时整个事务回滚。
- 管理 API 覆盖 PATCH 单字段更新、负数/小数/字符串/null/未知字段拒绝、原值保留、PUT 返回 405。
- 前端 `npm run check`、四语言扫描和演示构建通过。Chromium 演示 UI 默认 180/16384；1023 被拒绝；保存输出 24576、超时 600；请求体分别只包含对应字段。
- 挂起 PATCH 后输入、保存和取消禁用，按 Enter 不多发请求；800×600 视口无水平溢出，设置导航和卡片可操作，已查看截图。
- 演示 Vite 记录了 favicon 404 与 `ResizeObserver loop completed with undelivered notifications`，设置交互和小视口检查通过；不据此宣称整页无警告。

- macOS Tauri debug/no-bundle 构建与工作区 Clippy 通过，使用本机既有 SQLx 动态库参数 `--config 'profile.dev.package.sqlx-macros.strip="none"'`，未修改发布构建配置。
- 首次全量测试发现工具轮次夹具仍假定单批摘要；按新默认预留改为两批后，定向用例通过，验证同一个 Run 可在更多工具输出后再次完成自动压缩。

首轮 `cargo test --locked --workspace --all-targets`：634 通过、0 失败、14 忽略；同一工作区的 Clippy 与格式检查通过。忽略项需要外部环境，不视为验收。跨平台 CI 与合并发布结果在完成后补充。

这些是确定性 provider、数据库和演示 UI 验证，不是原作者 GLM 网关或真实 Cursor 长会话验收；本轮不运行 20 分钟任务，不改变 #155 的部分完成状态。

## 合并前云端审查

集成 head `8e93cf2cc9e0090fda7c91fb2028b019d951b241` 的 [PR CI](https://github.com/haohaomin/cursor-byok/actions/runs/45963408881) 和 [分支 CI](https://github.com/haohaomin/cursor-byok/actions/runs/45963333377) 均通过。Sourcery 因额度用尽跳过，不记为通过。

Codex 指出该版本把三次重试额度用于整个压缩过程：前一批第三次重试成功后，后续批次的首次截断就直接失败。该场景新增回归在修复前失败，修复后通过；将计数器在每批成功后归零，已经缩小的历史输入限额继续沿用，失败批次仍不能无限重试。验证完整 400K 字符答案在成功批次中逐字覆盖、已有摘要带入后续重试，截断内容不落库。最终提交与复验结果在完成后补充。
