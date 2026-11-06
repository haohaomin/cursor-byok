# Issue #134：从 BYOK 打开 Cursor

日期：2026-11-06。基线：`d7b370b508b030741f4898c925a4fbc7bdbf866d`。

上游 [Issue #134](https://github.com/leookun/cursor-byok/issues/134) 仍为 OPEN，要求增加打开 Cursor 的按钮，或在 BYOK 启动时自动打开 Cursor。本次实现手动按钮：在 Cursor 配置页的操作栏点击即可交给操作系统打开 Cursor。没有增加自动启动设置。

## 参考与实现

当前基线没有启动入口。检索上游打开 Cursor 的相关 PR 后，没有发现可直接复用的对应实现；已有终止 Cursor 的接管流程不适合这个操作。复用项目已安装的 Tauri opener 插件和系统 URI 关联，参考 [Tauri opener 文档](https://v2.tauri.app/plugin/opener/) 与 [Cursor deep links 文档](https://cursor.com/docs/reference/deeplinks)，没有新增依赖。

```text
apps/desktop/
├── src/features/models/
│   ├── OpenCursorButton.tsx          # 原生启动、等待状态、错误提示
│   ├── CursorSettingsPage.tsx        # 把按钮放入现有页面操作栏
│   └── CursorSettings.module.scss    # 长文案收缩；保留接管开关尺寸
├── src/shell/AppLayout.module.scss   # 将操作栏限制在内容区，保留右侧图标尺寸
├── src/shared/ui/icons.ts            # 启动动作图标
├── src/i18n/                         # 四种语言的按钮和错误提示
└── src-tauri/capabilities/default.json # 仅允许 opener 打开 cursor://
```

执行路径：用户点击 → 按钮暂时禁用 → `openUrl("cursor://")` → 操作系统 URI handler → 恢复按钮；opener 拒绝时显示错误。操作不依赖模型或 CA 初始化，也不更改代理、账号、接管状态或已有 Cursor 工作区。普通浏览器不显示该原生操作。

## 原生验收

使用隔离的 macOS Tauri 测试应用加载真实 React 源码、当前 capability 和真实 opener 插件；业务接口使用演示数据，不连接正式数据库。

| 场景 | 观察 |
| --- | --- |
| 原基线 | Cursor 页面没有“打开 Cursor”操作 |
| 新按钮，但尚未加入权限 | 点击后出现 `opener.open_url not allowed` 错误提示，按钮恢复可用 |
| 加入精确 `cursor://` 权限后 | 真实点击完成系统分发；验收窗口退到后台，已有 Cursor 窗口可读取，没有错误对话框 |
| 尝试其他 Cursor deep link | `cursor://file/issue134-probe-nonexistent` 被原生 scope 拒绝 |
| 设置导航 | 从 Cursor 页面实际点击进入系统设置成功 |

原生窗口捕获返回 Stage Manager 缩略图，测得 WebView 实际为 1083×558，不能把它写成 820×558 原生窗口截图验收。另在原生应用容器限定为 820×558 时测量了按钮位置和溢出，再以真实 820×558 Chromium viewport 完成以下布局测试。

## 布局问题与复测

测试发现俄语长文案会把接管开关挤到内容区外：未显示新按钮时，开关 x=208.296875，而内容区起点为 x=212；新增按钮会进一步压缩空间。已限制操作栏宽度，让长文案省略显示并保留完整标题提示，固定开关及图标按钮尺寸。

Playwright 使用真实页面和演示数据；仅为显示原生按钮模拟 Tauri 检测及版本/自启/更新查询，不用浏览器结果冒充操作系统启动验收。

- 中文、英文、葡萄牙语、俄语分别从设置菜单切换，`document.lang` 和按钮标签即时更新。
- 820×558 下所有操作栏控件留在内容区，开关宽 38，启动图标宽 25；按钮 y=40.5，避开 30 像素顶部区域。
- 四种语言均点击按供应商分组、恢复平铺成功；内容滚动 450 像素时，标题栏和启动按钮位置不变，document/root 不滚动。
- 检查深色及亮色呈现，设置导航可达；普通浏览器页面加载成功且启动按钮数量为 0。

结构化结果见 [验收证据](2026-11-06-issue134-open-cursor-evidence.json)。原始浏览器日志和隔离原生构建日志保存在本机 `/tmp/cursor-byok-issue134-20261106/`，截图保存在该目录的 `screenshots/`，不把临时 harness 加入产品。

## 构建与边界

- `npm --prefix apps/desktop run i18n:scan`、`npm --prefix apps/desktop run check`：通过。
- 原生 debug 构建通过：`CARGO_TARGET_DIR=/Users/renhao/works/cursor-byok/target npm --prefix apps/desktop run tauri:build -- --debug --no-bundle -- --config 'profile.dev.package.sqlx-macros.strip="none"'`。该命令行覆盖用于规避本机 Rust/SQLx 宏 dylib 的既有 LINKEDIT 错误，不修改仓库构建配置。
- `cargo fmt --all -- --check`、`git diff --check`：通过。

原生 opener 成功表示请求已分发给系统，不保证每个平台都能检测缺失的 URI handler。未退出用户 Cursor 来测试冷启动，未验收 Windows/Linux GUI 或未安装 Cursor 的操作系统行为。验收应用、浏览器会话和 Vite 服务已关闭；未替换正式安装包、修改 Cursor 安装文件或发布新版本。

集成复测：接入 main `3a6d50fbdba9e8a534b17e292d71f1bdd5d31ead` 的系统主题支持后，再次完成四语言浏览器场景、亮色截图、前端检查及原生 debug 构建，均通过；四种语言均为 473 个键，没有缺失或空译文。
