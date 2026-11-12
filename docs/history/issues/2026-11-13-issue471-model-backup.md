# Issue #471：模型配置 JSON 备份

2026-11-13 完成 Issue #471 的最小可用实现。用户可以在模型设置页把当前模型配置导出为 JSON，并在另一台电脑导入。JSON 方案满足“JSON 或 YAML”的需求，暂不引入 YAML 依赖或第二套格式。

## 结构

```text
apps/desktop/src/features/models/
└── CursorSettingsPage.tsx (490 行)       # 导出下载、文件选择、校验和结果提示
apps/desktop/src/shared/
└── api.ts (578 行)                       # ModelBackup 类型和导入请求
apps/desktop/src/demo/
└── api.ts (368 行)                       # Demo 导入路由
server/src/control/
├── models.rs (134 行)                    # 备份格式校验与导入路由
├── service.rs (1075 行)                  # 控制服务调用 Store 导入
└── mod.rs                                # 注册 POST /models/backup/import
server/src/store/
└── models.rs (414 行)                    # 复用现有规范化、哈希和重复跳过逻辑
server/tests/
└── external_api.rs (1053 行)             # 导入、重复和非法备份回归
```

行数包含空行和注释，不含生成文件。数据流为：

```text
设置页当前模型
    ↓ 去掉数据库身份字段 model_hash/时间戳
cursor-byok-models v1 JSON（包含 API Key，界面明确提示）
    ↓ 选择文件并解析
POST /__byok-api__/api/models/backup/import
    ↓ 校验 format/version → normalize → model_hash → 冲突跳过
SQLite model_configs → 刷新模型列表
```

导入使用已有 `create_models_if_missing`，不会覆盖当前同哈希模型，也不会重复插入同一备份。格式固定为：

```json
{
  "format": "cursor-byok-models",
  "version": 1,
  "models": []
}
```

版本号和格式不匹配、模型列表为空时服务端返回 400。客户端只接受本项目导出的 JSON；服务端仍会重新规范化模型并计算哈希。导出文件含 API Key，不能上传到公开仓库或发送给不可信的人。

## 验证

| 检查 | 结果 | 范围 |
| --- | --- | --- |
| `model_backup_import_skips_existing_models_and_is_idempotent` | 通过 | 已存在模型跳过；新增模型导入；重复导入返回 0 新增 |
| `model_backup_import_rejects_unknown_format_and_empty_backups` | 通过 | 非法格式和空备份均返回 400 |
| `cargo fmt --all -- --check` | 通过 | Rust 格式 |
| `git diff --check` | 已通过 | 差异无空白错误 |
| `npm run i18n:scan` | 已通过 | 生成词条；`en-US` 新增词条已补齐 |
| `npm run check` | 通过 | 桌面类型检查和生产构建；保留既有动态导入和大 chunk 警告 |
| `npm run tauri:build -- --debug --no-bundle` | 通过（隔离 target） | macOS Tauri debug/no-bundle 产物可生成 |
| 工作区 Rust 全量测试 | 293 通过、14 忽略、0 失败 | 现有全量回归；忽略项按项目配置需要 Deno 或缓存模型 |
| 工作区 Clippy | 通过 | `-D warnings` |

当前证据证明服务端导入逻辑、前端类型/构建和 Tauri debug 产物可用；尚未证明已安装的 Cursor 桌面包在真实文件选择器中完成一次导出再导入，也未验证跨操作系统文件选择器行为。该项保持部分完成，直到桌面人工验收补齐。
