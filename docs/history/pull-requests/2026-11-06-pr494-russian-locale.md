# PR #494：俄语桌面本地化适配

日期：2026-11-06

上游 PR：[leookun/cursor-byok#494](https://github.com/leookun/cursor-byok/pull/494)，固定 head `d44e5fe92451ce136018506f079488e90548a1fa`。

## 结论

已将 #494 的非插件本地化功能适配到当前 fork，并提交个人 fork [PR #18](https://github.com/haohaomin/cursor-byok/pull/18)。没有直接合入上游 README 和旧词条：当前 fork 默认中文 README、独立更新源、无广告界面和 Remote SSH 文案与上游不同，俄语 README 已按本 fork 的维护入口重写。

## 当前执行路径

```text
系统语言或设置选择
    ↓
apps/desktop/src/i18n/store.ts
    ├── 解析 ru / ru-* → ru-RU
    └── 持久化显式语言选择并更新 document.lang
    ↓
apps/desktop/src/i18n/runtime.ts
    └── 加载 ru-RU.json，向 t() 提供运行时译文
    ↓
apps/desktop/plugins/static-i18n-plugin.ts
    └── 扫描 469 个 source，检查目录完整性和占位符
    ↓
apps/desktop/src/features/settings/SettingsPage.tsx
    └── “Русский”选项与当前语言名称
```

相关文件的实测行数如下（包含空行和测试/生成数据；本项没有新增生成代码以外的测试文件）：

```text
apps/desktop/src/i18n/runtime.ts                 43  # 运行时语言类型和消息表
apps/desktop/src/i18n/store.ts                   78  # 系统语言解析和偏好状态
apps/desktop/plugins/static-i18n-plugin.ts      467  # 静态词条扫描与校验
apps/desktop/src/features/settings/SettingsPage.tsx 326 # 设置页语言选择
apps/desktop/src/i18n/locales/ru-RU.json         471  # 469 个俄语词条
README-RU.md                                    108  # 本 fork 俄语入口
```

上游目录只有 452 个俄语词条。当前 fork 的 source catalog 有 469 个词条；适配时删除了上游遗留的 8 个广告词条，补译了当前 fork 新增的 Remote SSH、插件恢复、图片移除和接管状态等 25 个词条。俄语目录最终与中文 source 逐键一致，没有空值或多余键。

## 验证

- `npm ci --prefix apps/desktop`：依赖安装完成。
- `npm --prefix apps/desktop run i18n:scan`：扫描并生成目录成功。
- `npm --prefix apps/desktop run check`：TypeScript、Node 配置检查和 Vite production build 全部通过。
- 独立目录校验：`zh-CN`、`en-US`、`pt-BR`、`ru-RU` 均为 469 个键，所有译文非空，占位符名称逐项一致。
- `git diff --check`：通过。
- PR #18 CI：Linux、Windows、macOS 和 Frontend 全部通过；Sourcery review 按仓库配置跳过。记录：[CI run 42105125861](https://github.com/haohaomin/cursor-byok/actions/runs/42105125861)。

构建仍会输出既有的动态导入和大 chunk 警告；没有新增错误。尚未使用真实 Russian 系统语言启动已安装 Tauri 包，需在 PR 合并或发布前做一次设置页切换确认。

## 范围边界

- 不修改插件实现；插件相关 PR 继续按用户要求延后。
- 不修改协议、服务端、数据库或发布签名配置。
- README 仅增加语言入口，并保留本 fork 的独立维护和免责声明；不把上游项目的旧社区入口当作本 fork 支持渠道。
