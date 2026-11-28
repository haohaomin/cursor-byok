# Issue #244：现代 Await 候选实现

日期：2026-11-28；基线 `e2fe7a7`（v1.2.15 后的发布记录提交）。本项承接已发布的通知接收和子代理状态修复，补齐等待已有任务的工具通路。候选已提交至 [PR #68](https://github.com/haohaomin/cursor-byok/pull/68)，未合入、未发布，真实 Cursor 未验收。

## 复现

工具目录以及子代理、Shell、无 ID 等待四项用例在基线全部失败。三个执行入口都立即返回 `Tool "Await" is not available`；不是任务已经运行后超时。原始日志 `/tmp/cursor-byok-issue244-await-red.log` 的摘要与 SHA-256 保存在[证据](2026-11-28-issue244-await-evidence.json)。

安装包审计见[客户端契约](2026-11-28-issue244-await-contract.md)。当前客户端有专用 SubagentAwait 执行器和 Await 卡片；旧 PR #245 的 Go 实现、旧工具别名和旧 Projects 模式不采用。

## 实现

```text
server/
├─ prompt/cursor/tools.json          # Await 的参数和模型使用说明
├─ prompt/cursor/modes/              # 已提供 Shell 的模式同时提供 Await
├─ src/cursor/tools/
│  ├─ await_task/mod.rs              # 输入校验、等待计时和执行所有权
│  ├─ await_task/shell.rs            # 客户端终端文件轮询、完成标记与 regex
│  ├─ await_task/output.rs           # 子代理状态、transcript 长度、统一结果
│  ├─ runtime.rs                    # 后台工具工作、延迟 Exec 队列和取消
│  ├─ codec/                        # Await 卡片、Exec 回包与断流处理
│  └─ semble.rs                     # 沿用通用工作所有权，搜索逻辑不变
├─ src/cursor/conversation/output.rs # 在现有 Run 循环发送延迟的只读请求
└─ tests/await_tools.rs              # 工具目录、协议往返、历史与用户中断

模型 Await
  ├─ 无 ID：本地计时 → 完成结果
  ├─ 数字 ID：客户端 Read → footer/regex/时限 → 必要时 250 ms 后再读
  └─ 子代理 ID：专用 Exec field 37 → 状态 → 可选 transcript Read
       → 一份 AwaitToolCall + 一份工具结果
       → 原有历史持久化 → 模型在同一工具轮次继续

新用户消息 / 停止 / 替换 Run
  → 取消等待工作，Abort 当前观察请求，丢弃过期的排队请求
  → 不启动、不恢复、不终止被观察的 Shell 或子代理
```

Await 追加在各模式原有工具列表末尾，保留既有工具的稳定前缀。默认等待 30 秒，最大 7,140,000 ms；任务 ID 为零等待时只检查一次。数字 ID 原样用于文件名，不能带路径分隔符；非数字 ID 使用专用子代理等待请求。子代理不接受 pattern；无 ID 的零等待和未知参数明确报错。

终端完成只依据原始文件末尾 footer，不能据此宣称检查过进程存活。regex 正文保留真实输出的尾部空白；无输出时紧接文件头的 footer 也可识别。运行时间采用完成 footer 或头部 running_for_ms 快照，并按原生容差处理 started_at 与服务端时钟差异。regex 只匹配去掉元数据的正文，匹配成功返回 still_running；匹配片段限制长度。截断或范围读取明确报错。子代理 transcript 读取失败保留客户端给出的状态，长度为 0。重复终态回包忽略，断流和错误回包成为 Await 工具错误。

## 与原生行为的边界

- 工具由本维护版本按模式提供，不把 protobuf 存在或 `long_running_jobs` 当成原生 Await 功能开关生效的证据。客户端缺少专用执行器时保留错误，不退回 Task resume。
- 本候选使用客户端给出的 transcript 路径，不增加旧执行兜底或多路径/LS 搜索恢复。
- 新用户消息沿用 BYOK 现有中断和续跑流程；取消等待者后处理新消息，不伪造原生同一生命周期内的 `wake_reason=context_injection` 成功结果。
- 复用项目已有 Rust regex。原生可选的“禁止阻塞已监控 Shell”开关没有可用的运行值证据；工具说明优先依赖自动通知，没有推断客户端订阅状态。
- 原生 Cursor 自动通知与 Await 的竞争、原生卡片、云子代理和真实供应商仍需验收。完整 #244、Projects 和 Agent Store 不标为已解决；不进行 20 分钟测试。

## 验证状态

最初四项失败用例已通过；三个完整协议往返和新用户消息中断均经 FakeProvider、临时数据库验证，旧模型历史保持结构前缀不变。定向单测另覆盖取消/延迟请求、完成 footer、正文匹配、重复回包、缺失任务、断流、截断与参数校验。

最终源码 `bce1466`：13 项 Await 单测、5 项 Await 集成测试和 13 项前缀测试通过；工作区 **706 通过、0 失败、14 忽略**，Clippy `--workspace --all-targets -- -D warnings` 和格式检查通过。原生契约的限定范围静态复核确认三项修正，无新增阻断项。最终云端 CI、审查、合入及发布尚未完成。所有测试输入均为合成数据，未修改正式代理、数据库或 Cursor 安装文件。

对照复核补充：原生纯函数与最初候选对比发现尾部换行丢失、空输出 footer 漏判；另补齐运行中任务时长。先复现两项运行回归失败，再修正解析。空输出格式是契约输入对照，未证明真实终端生产者一定生成这种间隔；[对照证据](2026-11-28-issue244-await-evidence.json)保留来源、候选哈希与测试输入。
