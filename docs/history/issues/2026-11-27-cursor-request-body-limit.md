# Cursor HTTP 请求体与解压大小限制

状态：PR #63 已合入 main；最终工作区 645 项回归、Clippy 和跨平台 CI 通过，v1.2.11 已发布并完成资产、签名和更新清单核验。真实 Cursor 超限 UI 尚未验收。

## 来源与复现

来源是 [PR #55 审查中独立记录的缺口](../pull-requests/2026-11-26-pr55-binary-append.md#审查与合入)，并非 #55 新引入的问题。基线为 main `894a482e9057d49e7d999c98305226dbda71aa95`（v1.2.10 后的文档提交）。

Cursor 路由关闭了 Axum 默认限制，处理函数使用 `to_bytes(body, usize::MAX)`；HTTP 解压发生在这些读取之前。二进制和十六进制 BidiAppend 共用这个入口，因此只限制内层 Agent protobuf 无法约束外层请求读取及 gzip 膨胀。

在未修改生产代码的基线上，四项新回归失败：

| 输入 | 基线行为 | 修复后的行为 |
| --- | --- | --- |
| `Content-Length = 64 MiB + 1`，body 始终未到达 | 等待读取，测试 2 秒超时 | 不读取 body，立即拒绝 |
| 无长度头，128 个 1 MiB 分块 | 全部读取后 protobuf 解析失败，HTTP 400 | 读到第 65 块拒绝，不再轮询后续块 |
| 小于 1 MiB 的 gzip 解压为 64 MiB + 1 | 全量解压后 protobuf 解析失败，HTTP 400 | 在解析前拒绝 |
| 普通 body 为 64 MiB / 64 MiB + 1 | 初轮探针测试均返回 200，未存在读取上限 | 最终边界测试改在消费正文的 Bidi 入口执行；前者到达 protobuf 解析层，后者返回大小错误 |

同期两项控制测试通过：原有压缩错误分类与 Tab/未知路径流式分派。测试使用临时 SQLite，大小载荷在进程内 router 验证，不调用真实模型；流式测试显式选择本地上游。

## 执行路径与范围

```text
已注册且消费正文的 Cursor 非 Tab 接口
  → request_body::buffer：检查长度头，最多读取 64 MiB 原始 body
  → tower-http RequestDecompressionLayer：按原有规则解压
  → request_body::buffer：最多读取 64 MiB 解压结果
  → 原处理函数：protobuf 解码 → 追踪/路由 → Run → 模型调用
  → 原响应流：保持流式输出

健康检查 / IsConnected → 忽略正文，立即响应
Tab / 文件同步 → 原解压层 → 原流式转发
未知路径 / 不支持的方法 → 原透明转发
```

```text
server/
├── Cargo.toml                             # 将已有 http-body-util 用于生产错误类型识别
├── src/api/cursor/
│   ├── handlers.rs                        # 路由分组与中间件顺序；421 行
│   ├── request_body.rs                    # 有界读取及明确的超限响应；54 行，新增
│   └── mod.rs                             # 注册私有请求体模块
└── tests/
    ├── cursor_request_body.rs             # 九项大小、压缩、探针和流式回归；335 行，新增
    └── cursor_large_append.rs             # 大历史和 PNG 到达模型的端到端回归；229 行，新增
```

行数包含注释和空行；测试单列，无生成代码。没有迁移、兼容路径或新增依赖包版本。限制属于 Cursor 注册接口，不作用于模型备份等管理 API。

64 MiB 是压缩前、解压后各自的请求体上限，包含 protobuf 外层和十六进制编码开销，不是模型 token 上限。它给多 MiB 图片和长历史留出空间，同时让单次缓冲有确定边界；没有声称所有大小的合法内容都能通过。超过上限明确拒绝，不截断后继续执行。单个请求的这两段缓冲有界，不等于整个进程只使用 64 MiB：protobuf 对象、后续历史处理及并发请求还有各自内存开销。

返回 JSON `resource_exhausted`，消息明确提示 64 MiB 和减少附件/历史；HTTP 使用 429，以符合 [Connect 错误码映射](https://connectrpc.com/docs/protocol/#error-codes)，不采用通用中间件的纯文本 413。其他读取/解压错误保留 HTTP 400，未知编码保留 415。

## 验证

- 新增大小/流式边界：9 项通过。除上表外，覆盖虚假的小长度头、压缩前超限、错误压缩、未知编码、Tab、文件上传、未知路径及方法 fallback。请求超限时没有模型调用。
- 新增大内容回归：1 项通过，内部覆盖四组二进制/十六进制、bare/Connect envelope、identity/gzip 组合。真实编码的 PNG 大于 3 MiB，历史文字大于 4 MiB；历史通过有效 checkpoint Blob 恢复，模型输入逐字节比较，没有截断、丢图或重复调用。
- 原 `connect_wire`：7 项通过，包括二进制 gzip 请求、KV 确认和完整 checkpoint 输出。
- 大内容测试使用 fake provider，并显式给测试模型足够上下文，避免把 HTTP 大小验收与模型 token 限制混淆；它不代表真实供应商接受如此长的上下文。
- 最终提交 `60dbe97` 的全量工作区 `cargo test --locked --workspace --all-targets`：645 通过、0 失败、14 忽略（原有 Deno/模型环境测试）；`cargo clippy --locked --workspace --all-targets -- -D warnings`、格式和差异检查通过。macOS 构建继续使用本机既有 SQLx 宏动态库参数，未改变仓库配置。
- 最终提交的 [PR CI](https://github.com/haohaomin/cursor-byok/actions/runs/45975007662)、[分支 CI](https://github.com/haohaomin/cursor-byok/actions/runs/45974998200) 及 [main CI](https://github.com/haohaomin/cursor-byok/actions/runs/45976277134) 均成功。
- Codex 代码审查覆盖初版 `6624f53`，没有发现问题；Sourcery 初版提出的两个建议已处理并以新回归验证。最终提交的 Sourcery 复审被跳过，不记作复审通过。

真实 Cursor 对超限错误的 UI 呈现尚未验收；本轮不扩大 #155 的验收范围，不测试 20 分钟任务。透明流式转发、上游响应大小与并发总内存不属于这个已确认入口缺口的修复范围。

## 审查补充

[PR #63](https://github.com/haohaomin/cursor-byok/pull/63) 的初版 `6624f53` 审查中，Sourcery 指出无正文健康检查被统一缓冲后会等待未结束的上传。新增回归在该提交上超时失败；最终将 healthz 和 IsConnected 移到缓冲层之外。它们从不读取、解析或存储正文，仍立即答复。相应 64 MiB 边界断言移至 Bidi 入口，使用不可解析正文区分“通过大小检查到达解析层”的 400 与超限的 429。

另一条建议是加强流式证明。保留原 URL 校验测试作为“路由未被缓冲层拦截”的证据，同时增加真实 loopback HTTP 上游：完整 Cursor router 使用自定义 Tab 目标；上传端发送 first 后保持打开，必须收到 first 响应，再发送 second 并收到 second；最后关闭上传并收到 EOF。覆盖 StreamCpp 和 FSUploadFile，两条均通过。没有改变上游 URL 的生产校验规则。

## 合入与发布

- [PR #63](https://github.com/haohaomin/cursor-byok/pull/63) 于 2026-11-27 合并；最终测试 head `60dbe97756cf7a3ca8ae71c7bee3de84e7f4fbe2`，合并提交 `845a3ac18be7a27e4a67b815c7d8d10a89dce50d`，两者树内容一致。
- 主线 CI 已通过；[自动发布流程](https://github.com/haohaomin/cursor-byok/actions/runs/45977506023) 已创建 `v1.2.11` 标签，指向 `e43a39a6ae9fd61b001dc0e15ec42028dbd658e6`。该提交仅更新五处版本号，四个平台及最终发布均成功；[v1.2.11](https://github.com/haohaomin/cursor-byok/releases/tag/v1.2.11) 已公开为 Latest，draft=false、prerelease=false。
- 已合并的本地和远端 `codex/cursor-request-body-limit` 分支已删除。

- 发布后实际下载 20 项资产，大小/SHA-256 全部匹配；7 份签名使用未变更的公钥验证有效，三个更新清单的版本、平台集合、URL、签名及旧版哈希/大小全部正确。[结构化证据](../2026-11-27-v1.2.11-release-evidence.json)。中文发布说明已补齐，未安装新包到用户机器。
