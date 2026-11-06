# #370：导入弹窗内容无法滚动

2026-11-06 在 main `73126d46bedac174e6449f2a45240a2955852bcb` 复现并修复。使用真实 React 页面、Chromium 和合成管理 API 数据；不读取正式配置或执行真实导入。

[上游 #370](https://github.com/leookun/cursor-byok/issues/370) 仍为 OPEN，正文和评论为空。搜索上游 PR 的 `scroll` 未找到现成补丁。原报告未说明具体导入入口，本记录只证明当前“导入旧版配置”预览存在下述缺口，不推断原作者的平台与完整触发条件。

## 复现与原因

820×558 窗口、没有已配置模型、预览返回 120 个模型。点击“导入旧版配置”后，弹窗外层正文只有 410 px 高，内部滚动视口却随内容扩展至 7212 px；视口自身没有溢出，外层直接裁掉剩余内容。滚轮输入后 `scrollTop` 仍为 0，第 120 个模型位于窗口外。

现有组件链仅用百分比高度传递尺寸，未让每层参与父弹性容器的收缩。本次在两层滚动容器使用纵向 flex，并允许实际 viewport 随可用空间收缩，沿用现有滚轮、滚动条及 ResizeObserver 实现。

```text
apps/desktop/src/
├── shared/virtual/ScrollableContent.module.scss  # 7 行：内容包装层向下传递可用高度
├── shared/scrollable/ScrollArea.module.scss      # 83 行：滚动视口在容器内伸缩
└── shared/ui/Modal.tsx                           # 未改：标题/按钮固定，正文使用共用滚动组件

预览响应 → LegacyModelImport → Modal 固定标题/底部
                                ↓ 剩余高度
                         ScrollableContent → ScrollArea viewport
                                                ↓ 滚轮/拖拽改变 scrollTop
                                           完整模型预览
```

行数包含注释和空行，不含测试或生成代码。没有新增组件、依赖、状态或业务 API，没有改动导入与持久化逻辑。

## 验证

同一回归脚本在基线报 `import viewport does not scroll`；修复后 viewport 为 410 px、内容仍为 7212 px，滚轮输入后 `scrollTop` 大于 0。恢复基线确认独立问题后，再恢复最终修改并重跑，此回归继续通过。

| 检查 | 结果 |
| --- | --- |
| 120 模型，滚轮/拖拽/轨道点击 | 通过；拖至底部 `scrollTop=6801.5`，第 120 个模型完整可见，点击轨道顶部回到 0 |
| 标题和确认/取消按钮 | 滚动前后矩形完全一致，底部始终在窗口内 |
| 820×558 ↔ 1200×800 | 通过；视口随高度从 410 调整为 652 px，标题保留 macOS 顶部 30 px 空间 |
| 1 个及 0 个模型 | 通过；弹窗随内容收缩，不塌陷、不强占最大高度、不产生多余滚动 |
| Tab / Shift+Tab / Escape | 两个底部按钮之间正确循环，Escape 关闭；关闭后的焦点恢复有既有缺口，见下文 |
| 共用调用表格 | 横向滚动 900 px、纵向滚动 320 px；表头、窗口顶部、右侧操作列固定；文档自身不滚动 |
| 共用调用详情页 | 558 px 视口展示 778 px 内容，滚轮能到达底部；文档自身不滚动 |
| 四语言主页面 | 设置语言选择及分组列表可操作；虚拟内容滚动、顶部保持固定；分组按钮换行问题见 #339 记录 |
| 模型编辑弹窗 | 数字字段可以滚入视口，固定底部按钮仍可达 |
| `npm --prefix apps/desktop run check` | 通过；保留既有 chunk 与动态导入警告 |
| Tauri debug / no-bundle | 通过；沿用本机 SQLx 宏 dylib 的构建参数覆盖，未修改仓库工具链配置 |
| `git diff --check` | 通过 |

构建命令：

```sh
CARGO_TARGET_DIR=/Users/renhao/works/cursor-byok/target \
  npm --prefix apps/desktop run tauri:build -- --debug --no-bundle -- \
  --config 'profile.dev.package.sqlx-macros.strip="none"'
```

结构化几何数据及用例结果见 [证据](2026-11-06-issue370-modal-scroll-evidence.json)。原始脚本、夹具、日志和截图保存在 `/tmp/cursor-byok-issue370-20261106/`。本轮没有真实旧版文件导入、正式数据库写入、原生 Windows/Linux GUI 或已安装 macOS 应用验收；浏览器数据不能替代这些范围。

## 独立待修问题

关闭异步加载的导入预览后，焦点回到 `BODY`，没有回到触发按钮。恢复未修改的基线后同样复现，属于已有焦点恢复问题，当前滚动修复未解决。不要把本轮 Tab 循环和 Escape 关闭通过记为焦点恢复通过。

[#339 的现版复测](2026-11-06-issue339-light-style-check.md) 发现英文、葡萄牙语的分组模型操作按钮在显示测试结果后换行；基线和当前候选均复现，另行修复。此提交不宣称所有弹窗或所有界面问题均已解决，也未发布桌面版本。

开发服务器还在分组列表展开/语言切换时记录了 `ResizeObserver loop completed with undelivered notifications`；首次出现在未修改基线，恢复基线复查时也出现。本轮没有把开发控制台记为无错误，该既有通知循环需随分组布局问题继续检查。

## 集成检查

PR #28 合入后，已将 main `3a5f51318279874805d99e8a57d4d2973826cbcc` 合入 [PR #29](https://github.com/haohaomin/cursor-byok/pull/29) 分支；冲突仅在 Issue 清单相邻记录，保留各自最新状态。组合代码的滚动失败回归、`npm run check` 与同参数 Tauri debug 构建再次通过。此前 head `0e390e23e9312f163c39f3f315230e9fa9ddefa9` 两套四组 CI 均通过，合并后的新 head 仍须等待其独立 CI；此记录不代表 PR 已合入。

后续核对：PR #29 最终 head `57078dc2c78cd3a7a8a77049b40bae9f822747ab` 的 PR/push 两套四组 CI 全部通过，2026-11-06 08:16 UTC 已合入 main `1de8e4258c7a37e421fe0fc2d2e19807cbc729ed`。未发布新版本，原生 GUI 与焦点边界保持原结论。
