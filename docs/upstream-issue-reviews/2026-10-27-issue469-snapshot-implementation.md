# Issue #469 / PR #498：工作区主机采集与独立搜索快照

**实现中，保留在 `codex/remote-semble` 开发分支，未合入 main、未发布。** 本轮接通了生产代码中的采集、过滤、传输、索引与取消流程；真实 Cursor SSH、Windows 执行及审批尚未验收，不能把 #469 标为全部解决。

基线：`23e1a7b8c7a2e38d694786af8e653ed595b3c123`。上游 PR #498 head 仍为 `c7114878c33076340c0c2f1e77ce4e7512d1960e`，未直接采用原补丁。原方案的三个缺陷见[先前复现](2026-10-27-issue469-semble-check.md)，原生工具替代方案的限制见[Ls/元数据实验](2026-10-27-issue469-inventory-check.md)。上游仓库本轮重新查询仍为 archived，未再次尝试发送评论。

## 实际执行路径

```text
模型 SembleSearch / SembleFindRelated（原始工具调用 ID）
  → 当前 Cursor 工作区路径校验
  → Cursor Shell Exec：主机元数据、ignore 文本、Git 排除结果
  → BYOK 规则选择器：先排除，再请求正文
  → Cursor Shell Exec：分批传输选中的原始字节（base64）
  → BYOK：核对路径、大小、批次及完整工作区指纹
  → 请求内存快照 → 原有切块、真实嵌入模型及检索排序
  → 原始工具调用收到一个结果，模型继续

取消 / 替换 run
  → 标记本次搜索取消，终止异步等待，向仍在执行的 Shell 发 Abort
  → 索引线程在文件边界检查取消标记
  → 丢弃迟到结果，释放快照；不创建新生命周期
```

HTTP(S) Git URL 仍走显式获取仓库的路径。文件系统路径统一由 Cursor 的工作区主机提供，避免把远端 `/workspace` 误当成本机同名目录。采集请求使用只读工作区策略、禁止网络，保留客户端审批；这是协议请求的限制，尚不能冒充真实客户端审批验收。

```text
server/src/
├── cursor/compile/run.rs               # 从请求取得工作区路径，不改历史消息
├── cursor/tools/semble.rs              # 一次工具调用拥有全部 Exec 与结果（312 行）
├── cursor/tools/runtime.rs             # 拥有取消标记、执行登记和异步任务句柄
└── search/snapshot/
    ├── mod.rs                         # 两阶段采集、分批、完整性校验（195 行）
    ├── command.rs                     # POSIX / PowerShell 启动与编码（112 行）
    ├── collect.py                     # 元数据、Git 规则、指纹和选定正文（245 行）
    ├── posix.py                       # 目录描述符与不跟随链接的读取（62 行）
    └── windows.py                     # Win32 句柄、物理路径和变更检查（172 行）
crates/semble-core/src/
├── source/inventory.rs                # 传输正文前执行排除策略（219 行）
├── source/snapshot.rs                 # 验证请求拥有的路径和字节（43 行）
├── index/builder.rs                   # 复用切块/嵌入，快照索引不写磁盘缓存
└── search/engine.rs                   # 共用检索逻辑，快照结果不回读服务器文件
```

行数为本轮实测，包含注释和空行，不包含独立测试与生成代码。现有本机索引 API 继续服务显式 Git URL、基准工具等用途；Cursor 文件系统搜索的旧直接读取服务器路径入口已替换。

## 过滤与隔离

- 主机 Git 计算 `.gitignore`、`info/exclude`、全局规则及嵌套仓库规则，`--no-index` 也检查已跟踪文件的排除状态。
- `.sembleignore`、`.ignore` 保留既有优先级；`.cursorignore` 独立生效，不能被 Semble 白名单重新纳入。父目录已排除的根或祖先目录不能通过选择其子目录绕过。
- 元数据阶段没有源码正文。固定忽略目录、类型、文件大小与规则先于正文请求生效。正文阶段只接受精确请求的文件；缺失、重复、额外文件、错误大小或指纹变化整体失败。
- 正文保留 CRLF 和非 UTF-8 字节；两份快照即使 `repo` 字符串相同也不共享文件或磁盘索引。大小写不同的远端名称保留在内存中。
- 采集不写工作区临时文件。POSIX 不跟随符号链接；Windows 使用 Win32 句柄检查链接与最终路径，代码仍需真实 Windows 测试。
- “不写快照磁盘缓存”不表示关闭应用原有的协议/请求日志。测试证据只使用合成文件。

## 已验证与待验收

| 验证 | 当前证据 |
| --- | --- |
| macOS 实际采集命令 | 7 项采集测试通过，其中 5 项执行实际命令；覆盖规则、原始字节、Unicode 140 文件多批、嵌套 Git、父目录排除、变化检测和链接 |
| 选择与快照自动化 | 6 项规则测试、4 项快照测试通过，包含同路径隔离、大小写、取消、路径检查与不写索引缓存 |
| Cursor 协议模拟 | 7 项通过：只读 Exec、拒绝、断流、重复/迟到、取消、等待线程中止、超限 Abort；不是实际 Cursor UI |
| 真实嵌入模型与实际本机 Shell | 显式运行 `real_model_host_exec_round_trip_search_and_related` 通过：两种工具各两次实际采集、结果回到原调用 ID、排除文件未请求；协议响应由测试驱动构造 |
| 全工作区 | 在增加超限用例与更新工具说明前，492 项通过；`SEMBLE_TEST_MODEL` / `SEMBLE_TEST_TOKENIZER` 指向实际缓存文件，模型夹具确实执行 |
| 前缀及最终静态检查 | 工具说明明确改为工作区主机路径，限制 `top_k ≤ 100`；旧工具摘要因此失败，已按新定义更新快照；前缀/恢复/压缩合计 23 项复测通过，最终全工作区 Clippy、格式及 diff 检查通过 |
| Windows / macOS CI | 已加入主机实际 Shell 采集、规则及快照测试；待开发分支 CI 结果，不以字符串启动测试代替 Windows 执行 |
| 真实 Cursor SSH / Windows UI、审批 | 尚未验收，未修改用户正式代理或安装程序 |

模型测试默认 `ignored`，避免普通 CI 隐式下载模型；手动执行：

```sh
cargo test -p cursor-server --lib real_model_host_exec_round_trip_search_and_related -- --ignored --nocapture
cargo test -p cursor-server --lib search::snapshot::tests
cargo test -p cursor-server --lib cursor::tools::semble::tests
cargo test -p semble-core --test inventory_policy --test request_snapshot
```

## 当前限制与合入门槛

工作区主机需要 Python 3.9+；Git 仓库需要可用 Git。POSIX 启动器可选择主机已安装的新版本 Python，不安装依赖。Windows 使用 PowerShell 和已安装 Python，编码后的命令有长度上限。

当前显式限制：20,000 个枚举项、每份规则 32 KiB / 总计 256 KiB、单源码文件 1,000,000 字节、选中正文总计 16 MiB；单批最多 128 文件 / 2 MiB。元数据超过限制或采集超过时间预算时报告失败，不返回成功的部分搜索。目录必须是物理路径；不把链接路径静默换成本机路径或其他目录。每次搜索重新采集并在内存建立索引，尚未证明大型工作区性能。

合入前仍需：通过 Windows 原生执行；真实 Cursor SSH 上完成两种查询、排除正文对照、取消及审批验证；确认客户端不会把内部采集文本误交给模型，且会正常显示原 Semble 工具结果。结果不足时继续保留开发状态，不发布为修复完成。
