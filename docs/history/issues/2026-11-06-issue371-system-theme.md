# Issue #371：默认跟随系统主题

## 结论与来源

[上游 #371](https://github.com/leookun/cursor-byok/issues/371) 要求默认跟随系统主题。2026-11-06 查询时仍为 OPEN，正文及评论为空；按 `theme` 搜索上游 PR 未找到现成补丁。

基线 `4e90dce850219947e945093de7ee246e85514198` 只有固定深色和浅色选项。真实 Chromium 中清除主题偏好、模拟系统浅色并刷新后，`prefers-color-scheme: dark` 为 false，但页面仍为 `default-dark`，已复现缺口。

本次沿用现有主题、store 和设置页面，参考 [Chrome 的系统主题处理说明](https://developer.chrome.com/docs/modern-web-guidance/guides/visual-design/dark-mode)：读取系统偏好并监听其变化。没有新增依赖。最终验证基于包含 Grep 修复的 main `d7b370b508b030741f4898c925a4fbc7bdbf866d`。

## 实现

```text
apps/desktop/
├── index.html                         # React 启动前解析偏好、设置首屏背景与色彩模式
└── src/
    ├── shared/theme/theme.ts          # 40 行；解析系统主题、管理唯一监听器、更新页面色彩
    ├── shared/store/appStore.ts        # 默认选择 system；只保存显式深色/浅色选择
    ├── features/settings/SettingsPage.tsx # 三个选项与可访问的选中状态
    └── i18n/                          # 中英葡俄主题文案；扫描生成词条目录
```

行数为实测，包含空行及注释，不含测试或生成代码。没有移动、合并或删除模块；删除了“默认暗色/默认亮色”旧文案。

```text
打开页面 → 读取本地偏好
  ├─ 显式深色/浅色 → 应用指定主题
  └─ 无偏好/无效值/存储不可用 → 读取系统主题
      ↓
React 初始化 → 同一主题模块接管
  ├─ system → 保留一个媒体查询监听器 → 系统变化时立即更新 DOM
  └─ 手动选择 → 移除监听器 → 保存选择 → 应用指定主题
      ↓
重新选择 system → 删除保存项 → 立即解析系统主题并重新监听
```

`data-theme` 始终是现有的 `default-dark` 或 `default-light`，不把 `system` 传入样式。同步更新 `color-scheme`、首屏背景变量与 `theme-color`。已保存的显式选择继续生效；系统变化不会覆盖它。存储写入失败时，选择仍在当前页面生效。

主题使用独立的“跟随系统主题”翻译，避免复用语言选择器中的英文 “Use system language”。

## 验证

浏览器使用当前应用代码与现有 `src/demo/api.ts`，临时入口仅替换 API 数据来源；真实管理服务、用户数据库和操作系统主题设置均未修改。结构化结果见 [验收证据](2026-11-06-issue371-theme-evidence.json)。

| 检查 | 结果 |
| --- | --- |
| 修复前系统浅色、无保存偏好 | 仍为深色，缺口复现 |
| React 模块加载前的 7 组检查 | 通过：系统浅色/深色、两种显式选择、system 值、无效值、存储读取被拒绝 |
| 10 次运行时状态核对 | 通过：默认系统主题、实时切换、手动浅色/深色不受系统变化覆盖、两种手动选择刷新保留、恢复 system 清除保存项、system 刷新后继续生效 |
| 设置页与翻译 | 中英葡俄主题文案显示正确；主题按钮具有正确的 `aria-pressed` 状态 |
| 最小窗口 820×558 | 深浅截图检查通过；三项主题选择与导航可达，内容在原滚动区域内，macOS 顶部预留区域不受影响 |
| `npm run i18n:scan`、`npm run check` | 通过；四种语言均为 471 个键，无空译文；构建保留既有大 chunk 警告 |
| Tauri debug / no-bundle 构建 | 通过，使用下述本机工具链参数 |
| `git diff --check` | 通过 |

本机 Rust 1.94.1 / macOS 27 的普通调试构建在加载 `sqlx_macros` 时失败，报 `mis-aligned LINKEDIT string pool`。清理该包缓存后仍复现；详细 rustc 命令显示该包启用了 `strip=debuginfo`，与 [Rust #157750](https://github.com/rust-lang/rust/issues/157750) 一致。仅对本次构建关闭该包的裁剪后构建成功，未修改仓库配置或系统工具链：

```sh
cd apps/desktop
npm run tauri:build -- --debug --no-bundle -- \
  --config 'profile.dev.package.sqlx-macros.strip="none"'
```

浏览器中的系统主题由 Playwright 模拟；没有把它称为已安装 Tauri 应用、原生系统主题切换或 Windows/Linux GUI 验收。演示 API 不包含真实账号与管理 API 验证，早期出现的 favicon 404 及演示端点缺失不作为主题行为的测试结果。此变更不涉及插件、Cursor 协议或数据库；没有发布桌面版本。

## 合入记录

[PR #26](https://github.com/haohaomin/cursor-byok/pull/26) 于 2026-11-06 合入 main，提交 `3a6d50fbdba9e8a534b17e292d71f1bdd5d31ead`。最终 head `a4eb3a1402df80e180ef57f6b210ecd52fd6b458` 的两套 CI 均通过；合并后的 [main CI](https://github.com/haohaomin/cursor-byok/actions/runs/42157993648) 也通过。合入不扩大上述实机验收范围，未发布新版本。
