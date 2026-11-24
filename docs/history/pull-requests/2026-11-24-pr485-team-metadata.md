# PR #485：本地身份的团队配置路由

日期：2026-11-24。来源：[lawyer112 的上游 PR #485](https://github.com/leookun/cursor-byok/pull/485)，固定 head `49e29e3756e1d2c0589281ad0527f7259b9e6887`。重新核对后仍开放、12 个文件、无 CI、评论或 reviews。已读取完整补丁，按独立功能选择采用；作者实机声明不当作本 fork 的验收证据。

## 问题与范围

本地 BYOK 身份没有 Cursor 团队，但两个明确允许“无团队返回空”的 RPC 没有注册本地处理：

- `/aiserver.v1.DashboardService/GetTeamReposOrEmptyIfNotInTeam`
- `/aiserver.v1.DashboardService/GetTeamAdminSettingsOrEmptyIfNotInTeam`

请求落入官方转发，内置本地身份无法由官方服务器识别。原实现的生产 Router 测试已失败：设置不可转发的测试上游地址后，本地请求返回转发校验错误 400，预期为本地空 protobuf 200。该失败证明路由遗漏，没有实测或假定官方服务实际返回的错误码。

候选采用原 PR 两条路由与团队空响应 handler，复用现有精确身份判定。非内置身份继续转发，真实团队账号的策略不被本地空响应覆盖。没有更改身份生成、账号信息、权限或订阅配置。

```text
server/src/
├── api/cursor/handlers.rs               注册两个指定 POST RPC
├── api/cursor/team_tests.rs             经生产 Router 验证响应、身份和转发边界
└── cursor/services/compatibility.rs     精确本地身份返回空 protobuf，其他身份转发
```

```text
指定 POST 团队接口
  → 内置本地身份：读取完请求体 → 200 application/proto，长度 0
  → 其他身份：原有官方代理
其他接口或 HTTP 方法 → 原有官方代理
```

本 fork 已将全部非 Tab Cursor 请求送入本地 Router，因此不重复移植原 PR 的代理白名单改动。Todo 已由 #52 单独合入；子代理与 DeepSeek 等其余内容继续单独评估。

## 验证

- 失败基线：本地团队接口 400 而非 200；非本地身份的转发边界测试通过。
- 两项生产 Router 回归覆盖两个 RPC、本地请求体消费、无 Connect 帧的空 protobuf、精确本地 token、被改动的 token、普通账号、无认证、GET 及未选择的团队接口。
- 使用不可转发的上游地址识别代理分支；没有联系官方服务、使用真实账号或修改桌面数据。
- 服务端单元测试 **309 通过、0 失败、14 忽略**，服务端全目标 Clippy（`-D warnings`）、格式与差异检查通过。跨平台 CI、合入与发布待确认。
- 原生 Cursor 启动、真实团队账号策略和完整 Windows Agent 会话尚未验收，不能由此把此前 Windows Agent 问题标记为已解决。
