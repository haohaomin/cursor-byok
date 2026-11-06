# 截断按钮首次操作丢失：复现与修复

2026-11-06，在 Issue #339 模型卡片回归中发现独立交互缺口：按钮文字被截断时，第一次鼠标点击或键盘 Tab → Enter 不执行操作，焦点也丢失。基线 `3a5f51318279874805d99e8a57d4d2973826cbcc` 已复现；修复在包含 PR #29 的 main `1de8e4258c7a37e421fe0fc2d2e19807cbc729ed` 上验证。

## 原因与参考

俄语“Проверить”的文字宽度为 73 px，可见宽度为 41 px。旧 `TruncatedButton` 首次悬停/聚焦时设置状态，将组件树从 `Button` 换为 `TooltipTrigger → Button`。原 DOM 按钮被移除，后续点击/键盘事件无法完成。两组基线都记录到 `nodeConnected=false`、焦点丢失、零次 POST，没有进入测试中状态。

已核查引入此组件的提交 `a5bbe67` 及[上游 PR #382](https://github.com/leookun/cursor-byok/pull/382) 的完整文件补丁；上游采用相同的条件包装方式，没有可直接采用的修复。[React 的组件身份说明](https://react.dev/learn/preserving-and-resetting-state)也说明，更换树中同一位置的组件类型会重建其下方内容。

## 修改范围

```text
apps/desktop/src/shared/ui/
├── TruncatedButton.tsx  # 22 行；保持按钮树不变，在交互时检查文字是否截断
└── TooltipTrigger.tsx   # 96 行；按检查结果显示提示，并转发操作及关闭提示
```

行数包含空行与注释，不含测试或生成代码。删除仅用于条件包装的状态及覆盖调用方事件处理器的测量逻辑；保留现有 Floating UI 定位和提示 Provider。鼠标移动及键盘聚焦时读取实际文字宽度，避免提前测量字体。点击时转发原回调并关闭提示，使 Enter/空格激活后不残留旧标签。

```text
指针移动 / 键盘聚焦
  → 读取当前文字宽度
  → 截断时显示完整提示，按钮节点始终保持
鼠标点击 / Enter / 空格
  → 原 onClick 执行一次
  → 关闭提示
```

## 验证

- 原失败回归：俄语首次鼠标点击、Tab → Enter 均通过；原按钮节点和焦点保留，各发出一次 POST。
- 实际模型页面：820×558 下，英文、葡萄牙语、俄语、中文 × 鼠标/键盘，共 8 组通过。每组测试后再取消，请求顺序均为一次 POST、一次 DELETE；没有重新挂载按钮或丢失焦点。此矩阵无未处理页面异常。
- 独立组件夹具复用真实组件及样式：验证完整提示、视口内定位、Escape/离开/点击关闭、Enter/空格激活、短标签不显示提示、禁用按钮不执行且跳过 Tab、调用方 focus/pointer-enter 回调以及普通 TooltipTrigger 的操作转发。
- 820×558 → 1200×800 → 820×558 缩放后移动指针，提示按新的截断状态关闭/重新显示，原按钮节点保持。没有声称静止指针下立即重测截断。
- `npm --prefix apps/desktop run check` 与 Tauri debug 构建结果见同名结构化证据。构建沿用本机 SQLx 宏工具链参数 `profile.dev.package.sqlx-macros.strip="none"`，未修改仓库工具链或构建配置。

四语言矩阵首次脚本读取页面启动时的临时语言过早，导致选择器超时；改为等待真实页面出现后读取语言，最终 8 组通过。保留失败脚本日志，不计为产品失败。

## 边界

浏览器使用真实 React 页面与合成管理 API，测试请求不访问真实供应商；独立组件夹具不模拟组件实现。未验收已安装 macOS 应用或 Windows/Linux GUI，未操作正式账号或数据库，未发布版本。本修复不代替分组模型布局 PR #30，也不表示 Issue #339 的其他平台样式问题全部关闭。

结构化结果见[测试证据](2026-11-06-truncated-button-interaction-evidence.json)。原始脚本、合成夹具、日志和截图保存在 `/tmp/cursor-byok-truncated-button-20261106/`。

## 与分组布局修复组合验证

PR #30 合入后，整合 main `574f2f4f3da4eef6b980b70931a2d256d81a8952`，冲突仅在 Issue #339 清单行。两个按钮组件的 SHA-256 与原验收源码一致；组合代码的 8 组首次交互及 12 组、60 次分组布局状态检查均通过，类型/生产构建及相同参数的 Tauri debug 构建再次通过。Playwright `pageerror` 计数为 0，但开发服务器仍记录既有 ResizeObserver 循环通知，不代表全应用无异常。更新后的 PR #31 仍需独立 CI，未发布版本。

后续核对：PR #31 最终 head `cbac599da05705abe1f6cb486f215fb0b955de6b` 的 PR/push 两套四组 CI 全部通过，2026-11-06 09:20 UTC 已合入 main `6602157d512d01591567bb00e561fef3a2cceb0e`。原生 GUI 等边界保持原结论，未发布版本。
