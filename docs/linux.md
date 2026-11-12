# Linux 安装与依赖

Cursor BYOK 的 Linux 桌面版使用 Tauri 和 WebKitGTK。Linux 安装包会使用宿主机的图形运行库；安装成功不代表发行版已经提供全部运行时依赖。

## 选择安装包

- **Debian / Ubuntu**：优先使用 `.deb`，由包管理器处理已声明的运行时依赖。
- **Fedora / Rocky / RHEL**：使用 `.rpm` 前，先确认启用的软件源能提供 `libwebkit2gtk-4.1.so.0`。不同发行版的 RPM 包名可能不同，不要只按包名猜测。
- **AppImage**：适合无法使用 RPM/DEB 包管理器的场景。它仍依赖兼容的桌面、GTK、显示服务器和系统权限，不能保证绕过所有发行版差异。

当前发布工作流在 Ubuntu 22.04 上构建 Linux 产物；Rocky Linux、RHEL 和 Fedora 不在发布矩阵中。它们的兼容性需要在目标版本上单独验证。

## 安装前检查

在 RPM 系统上，先查询哪个启用的软件源提供 WebKitGTK 运行库：

```bash
dnf provides '*/libwebkit2gtk-4.1.so.0'
```

如果查询没有结果，当前启用的软件源不能满足 RPM 的运行时依赖。此时应改用 AppImage，或先配置发行版提供的兼容仓库；不要用无关的旧版 WebKitGTK 库强行替代。

下载 RPM 后，也可以查看它声明的依赖：

```bash
rpm -qpR Cursor.BYOK-<version>-1.x86_64.rpm \
  | grep -E 'webkit|gtk|appindicator|rsvg'
```

安装后可确认动态链接器是否能找到 WebKitGTK：

```bash
ldconfig -p | grep 'libwebkit2gtk-4.1.so.0'
```

Debian / Ubuntu 上可按发行版文档安装对应的 WebKitGTK 4.1、Ayatana AppIndicator 和 librsvg 运行库；具体包名随发行版版本变化，应以 `apt` 的候选版本为准。

## Rocky Linux 9 / RHEL 9

上游 [Issue #486](https://github.com/leookun/cursor-byok/issues/486) 报告 Rocky Linux 9.5 安装 RPM 时缺少 `libwebkit2gtk-4.1.so.0`。本仓库已把依赖和支持边界写清楚，但尚未在 Rocky/RHEL 主机上完成安装、启动和托盘回归，因此不把该问题标记为彻底解决。

反馈 Linux 安装问题时，请附上以下信息：

```bash
cat /etc/os-release
uname -m
dnf repolist 2>/dev/null || true
ldconfig -p | grep -E 'webkit2gtk|appindicator|librsvg' || true
```

不要把 API Key、插件凭据或其他敏感配置贴到 Issue 中。
