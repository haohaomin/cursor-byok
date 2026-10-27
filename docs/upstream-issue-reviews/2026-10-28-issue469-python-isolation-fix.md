# Issue #469：工作区采集器的 Python 导入隔离

**已复现并修复开发分支中的独立缺口：采集命令会导入工作区或 `PYTHONPATH` 内的同名 Python 模块，导致搜索失败，并执行本不应作为依赖加载的项目代码。** 这不是 #469 原始代理连接或 CA 报错的根因结论。远程 Semble 仍保留开发状态，真实审批与 Windows Cursor UI 尚未验收，未合入 main、未发布。

生产代码基线：`d89d52a2fdc1f77822c69c836a563561795d1daf`。本轮重新读取上游 #469 正文与评论，GitHub 状态仍为 OPEN；原报告是 SSH 代理连接和证书错误，没有报告本次发现的 Python 导入问题。

## 失败基线

真实 Shell 请求将 `working_directory` 设置为模型选择的工作区，但此前主机测试没有切换到这个目录。Python 的默认启动方式将当前目录加入模块搜索路径，也会读取 `PYTHONPATH`。启动代码及采集器使用 `base64`、`json` 等标准库名称时，可能加载项目中的同名文件。

新增两项原生命令回归，先在未修复的生产代码上运行：

| 场景 | 预期 | 基线结果 |
| --- | --- | --- |
| 工作区含 `base64.py`、`json.py`、`sitecustomize.py`，并用 `.cursorignore` 排除 `*.py` | 两阶段采集只返回 `public.rs`，不执行被排除的模块，不生成工作区缓存 | 元数据阶段失败；堆栈指向工作区 `base64.py`，出现 `RuntimeError: WORKSPACE_CODE_EXECUTED` |
| `PYTHONPATH` 指向隔离的合成目录，其中含同名 `base64.py` | 完成采集，环境目录内代码不作为采集依赖执行 | 元数据阶段失败；堆栈指向环境目录 `base64.py`，出现 `RuntimeError: ENVIRONMENT_CODE_EXECUTED` |

基线命令为 `cargo test --package cursor-server --lib search::snapshot::tests::host::real_host_does_not_import -- --nocapture`，macOS / Python 3.9.6 上结果 **0 通过、2 失败**，执行耗时 0.07 秒（不含编译）。所有文件均为临时合成夹具，失败时也由临时目录所有者清理。

## 修改与验证

```text
server/src/search/snapshot/
├── command.rs  # POSIX / Windows 的版本检查和采集都用 Python -I -S
└── tests.rs    # 在实际工作目录执行命令，验证同名模块和环境路径不被导入
```

`-I` 隔离当前目录、用户模块路径与 `PYTHON*` 配置；`-S` 跳过 site 初始化及其启动钩子。采集器只依赖标准库，没有增加依赖、临时脚本或工作区写入。两个平台均在版本检查和正式执行时使用同一组参数，避免检查阶段先加载项目的 Python 配置。

修复后本机 `cargo test --package cursor-server --lib search::snapshot:: -- --nocapture` **14 项通过、0 项失败**。两项新增用例均实际执行生成的主机命令，完成元数据和正文两阶段，检查返回路径以及未产生 `__pycache__`；工作区同名模块用例还检查顶层文件数量没有变化。既有分页、大正文、Unicode、多批、过滤、链接与变化检测同时通过。

完整回归首次还暴露一项与采集器无关的环境问题：`provider::attempt::tests::request_transport_failure_is_one_failed_attempt` 期望连接失败，但实际经本机代理得到 `Provider("test 502 Bad Gateway: ")`；单独复跑得到相同结果。三个 HTTP attempt 夹具改为 `no_proxy()` 直连自己创建的本机测试端口，修复后均通过。该修改仅在测试模块内，不改变生产供应商请求的代理选择。

本机完整检查：

- `cargo test --package cursor-server --all-targets`：**447 项通过、0 项失败、1 项忽略**。忽略项需要显式提供真实嵌入模型，本轮未执行。
- `cargo clippy --package cursor-server --all-targets -- -D warnings`：通过。
- `cargo fmt --all -- --check`、`git diff --check`：通过。

本提交的跨平台 CI 结果在完成后补记。自动化不能代替真实 Cursor 的审批操作；本轮没有发送 Agent 请求、改动正式代理或生产数据，也没有执行 20 分钟任务。
