# Issue #254：现版已支持 TAB 自定义 base URL

2026-11-06 核查结论：现版已有设置入口、地址持久化和 TAB 转发实现，本次无需新增生产功能。新增 4 项集成测试并执行设置页验证，均通过。上游 [#254](https://github.com/leookun/cursor-byok/issues/254) 仍为 OPEN；本次没有关闭或评论上游 Issue。

Issue 正文为空，标题要求允许配置 TAB server 的 base URL。作者另提到控制面账号；本次没有账号环境，不将路由测试视为账号兼容性验收。相关 [PR #377](https://github.com/leookun/cursor-byok/pull/377) 已关闭且未合并，处理的是直连时恢复官方账号，不是缺少自定义地址入口，未采用其旧账号恢复补丁。

## 既有实现与本次修改

```text
apps/desktop/src/features/settings/
├── TabSettingsCard.tsx     # 90 行；显示公益/直连/自定义选项及地址输入，未修改
└── SettingsPage.tsx        # 327 行；保存 TAB 设置，空地址拦截和响应回填，未修改
server/src/
├── control/settings.rs    # 188 行；提供 TAB 设置 GET/PUT 接口，未修改
├── store/settings.rs      # 689 行；校验并持久化 TAB 模式和地址，未修改
├── cursor/services/tab.rs # 51 行；16 个 TAB RPC 每次读取当前配置，未修改
└── api/cursor/proxy.rs    # 227 行；追加原路径并流式转发请求/响应，未修改
server/tests/
└── tab_routing.rs         # 290 行；新增，验证配置持久化和真实 loopback HTTP 转发
```

所列行数包含空行与注释，不含生成代码；测试文件单独列出。沿用现有 Store 测试夹具、Axum、reqwest 和 Tokio，没有新增依赖或修改数据库结构。

```text
设置页提交 { mode: custom, address }
  → 管理接口 → Store 校验/持久化
TAB RPC → 读取当前配置
  → 自定义 base URL + 原始 path/query
  → 真实 HTTP 服务 → 状态、头与流式正文返回
```

服务端测试基于 `574f2f4f3da4eef6b980b70931a2d256d81a8952`；随后整合 main `6602157d512d01591567bb00e561fef3a2cceb0e`，`server/src` 无差异，设置页测试在该主线组合上执行。

## 服务端证据

4 项测试全部通过，无忽略项；测试目标 Clippy 和工作区格式检查通过。

| 用例 | 实际覆盖 |
| --- | --- |
| 持久化与 16 个 RPC | 保存带空格及尾部斜杠的地址，关闭原 Store 并重新连接 SQLite；16 个 RPC 均到达自定义路径前缀，保留 query、二进制正文及合成授权头，替换 Host，移除内部 upstream 指示头；响应状态、类型、业务头及字节保持 |
| 地址切换 | 同一个 Router 在配置从服务 A 改为 B 后，下一次请求到达 B；切换 Direct 后不再访问已保存的自定义服务 |
| 流式响应 | 接收端先发送首帧并等待通知；BYOK 返回首帧后测试才放行尾帧，证明没有等全部响应结束再交付 |
| 无效配置 | 空值、无法解析的地址、FTP、缺主机、query 和 fragment 均拒绝，最后一次有效配置保持不变 |

所有服务绑定 `127.0.0.1:0`，数据来自临时 SQLite。显式测试代理配置使用既有 loopback bypass，不更改进程环境、系统代理或用户设置。Direct 分支以无效的 upstream 头验证其路由选择，不访问官方服务；不据此宣称官方 TAB 已验收。

执行：`cargo test -p cursor-server --test tab_routing`、`cargo clippy -p cursor-server --test tab_routing -- -D warnings`、`cargo fmt --all -- --check`。本机沿用 SQLx 宏参数 `profile.dev.package.sqlx-macros.strip="none"`，未修改仓库工具链。

## 设置页证据与限制

真实 React 页面、Chromium、英文、820×558：空地址不发请求；键盘 Enter 保存时去掉首尾空白；正常接收规范化地址；保存失败保留编辑状态，取消后仍显示原值；Direct 隐藏地址，切回 Custom 保留已保存地址。共 3 次成功和 1 次失败的受控 PUT，无横向溢出。

设置页使用合成管理 API；SQLite 持久化和实际 HTTP 转发由独立 Rust 测试覆盖。没有完整贯通“真实 Cursor → 本地接管代理 → 外部 TAB 服务”的实机测试，没有 HTTPS 服务证书或控制面账号验收，也没有已安装原生应用验收。本次为既有功能补测试和记录，不是新生产修复，未发布版本。

首次 UI 选择器同时选中了外层虚拟页面 section；收窄到 TAB 卡片后测试通过，失败不计为产品缺陷。结构化结果见 [证据](2026-11-06-issue254-tab-baseurl-evidence.json)；原始日志、夹具、脚本和截图保存在 `/tmp/cursor-byok-issue254-20261106/`。
