# Issue #119：同一版本不重复自动提醒

日期：2026-11-06。基线：`3a6d50fbdba9e8a534b17e292d71f1bdd5d31ead`。

[上游 #119](https://github.com/leookun/cursor-byok/issues/119) 仍为 OPEN，要求关闭更新提示后不再重复弹出，并保留手动更新入口。原截图是 0.0.31 的旧界面；现版已经改为 6 秒自动消失的消息和设置页更新按钮，不再使用截图中的布局。

按更新通知关键词检查上游 PR，没有发现同一问题的独立补丁。[PR #502](https://github.com/leookun/cursor-byok/pull/502) 改的是旧 Go/Wails 界面的全局更新弹窗，与当前 React/Tauri 消息状态不同，未采用。

## 复现与修改

在真实 Chromium 中运行当前 React 源码，以演示管理接口和模拟 native updater 返回 `1.2.0`。首次提醒自动消失后刷新页面，**同一版本再次提醒**。聚焦回归在基线报 `same update notice repeated after reload`，修改后原场景通过。

当前进程内已有 `availableVersion` 去重，但页面重新创建后该值会重置。本次只在自动提醒处记住最后提醒的版本：

```text
apps/desktop/src/
├── App.tsx                          # 自动检查完成后读取/保存最后提醒的版本
├── shared/store/updateStore.ts      # 继续保存可安装更新，行为不变
└── features/settings/
    └── AppLifecycleSettingsCard.tsx # 保留手动检查和下载入口，行为不变
```

```text
自动检查得到可用版本
    ├─ 已提醒同一版本 → 不弹消息；设置页仍可更新
    └─ 不同版本 → 保存 cursor-byok.last-notified-update → 显示原有 6 秒消息
```

不改检查频率、更新源、签名校验、下载和安装流程。沿用前端偏好的 localStorage；读写失败时不阻断更新查询，已有内存状态仍避免本次会话反复提醒。

## 验证

| 场景 | 结果 |
| --- | --- |
| 初次发现、消息消失、刷新页面 | 初次提示正常，刷新后不重复 |
| 同一版本的下一次自动检查 | 推进 6 小时，第二次检查实际发生，没有重复消息 |
| 新版本 `1.3.0` | 再次提醒，保存项更新为 `1.3.0` |
| 设置页手动更新 | 版本仍可见，“下载并安装”启用；未点击安装 |
| localStorage 读取被拒绝 | 新版本正常提醒，本次会话后续检查不重复 |
| localStorage 写入失败 | 新版本正常提醒，本次会话后续检查不重复 |
| 820×558 最小窗口 | 设置导航和更新按钮可达；按钮位于 x=688、y=266.5、97×24 |
| i18n 扫描、`npm run check` | 通过；没有新增文案，只更新扫描记录行号 |
| 原生 debug / no-bundle 构建 | 通过；使用既有本机 SQLx 参数，见下方 |
| `cargo fmt --all -- --check`、`git diff --check` | 通过 |

构建命令：`CARGO_TARGET_DIR=/Users/renhao/works/cursor-byok/target npm --prefix apps/desktop run tauri:build -- --debug --no-bundle -- --config 'profile.dev.package.sqlx-macros.strip="none"'`。覆盖参数只用于本机既有 Rust/SQLx LINKEDIT 问题，没有写入项目配置。

结构化结果见 [验收证据](2026-11-06-issue119-update-notification-evidence.json)。测试时钟由 Playwright 推进，6 小时不是实钟等待；没有真实更新下载、安装或签名验证。未验收正式安装应用重启，localStorage 的记忆范围是同一个前端 origin；更换服务端口或清空 WebView 数据后可能再次提示，不能宣称跨 origin 的全局永久忽略。

测试浏览器、Vite 和临时入口已清理，原始日志与截图在 `/tmp/cursor-byok-issue119-20261106/`。未修改正式用户数据，也未发布版本。

集成复测：接入 main `73126d46bedac174e6449f2a45240a2955852bcb` 后，重复提示回归、820×558 的手动更新入口、i18n 扫描、前端检查和原生 debug 构建再次通过。
