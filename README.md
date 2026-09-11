# 剪贴板历史 (Clipboard History)

基于 Tauri 2 的剪贴板历史记录应用：启动后自动监听剪贴板，**每次有新内容进入剪贴板，
都会换行追加到历史记录中**（列表底部实时出现新条目）。

## 功能

- 自动监听：Windows 下通过剪贴板序列号实时检测变化（50ms 级响应），其他平台 500ms 轮询；
  新文本自动追加到历史列表（最新在底部，自动滚动）
- **重新复制相同内容也会记为新记录**（Windows：确认变化事件而非简单轮询）
- 复制图片 / 文件等非文本内容时会提示「仅文本会被记录」，不会无声无息
- 点击任意记录即可重新复制到剪贴板（自己写入的内容不会被重复记录）
- **全部复制**：一键把全部历史按时间顺序、每条换行合并复制到剪贴板
- 监听开关：可随时暂停 / 恢复监听
- 搜索过滤（Ctrl+F）、一键清空（需二次确认）
- 历史持久化：保存在系统应用数据目录 `history.json`，重启不丢失
- 驻留托盘：关闭窗口后最小化到系统托盘继续监听，托盘右键可退出
- 单实例：重复启动会唤出已有窗口，避免两个监听进程重复记录

## 运行

```bash
cargo tauri dev      # 开发模式
# 或直接：
cargo run
```

打包安装包：

```bash
cargo tauri build
```

## 说明

- 仅记录**文本**内容；图片、文件等非文本内容会被跳过（界面会给出提示）
- 每次复制动作（包括重新复制相同内容）都会记为新记录；自己点记录复制的内容不会重复记录
- 历史最多保留 1000 条，超出后自动丢弃最旧记录
- 历史数据位置：`%APPDATA%\com.aentrance.clipboardhistory\history.json`
  （Linux: `~/.local/share/...`，macOS: `~/Library/Application Support/...`）

## 项目结构

```
├── src/main.rs          # 后端：剪贴板轮询、持久化、托盘、Tauri 命令
├── capabilities/        # 权限配置（core:default）—— 必须存在，否则前端事件监听会被 ACL 拒绝
├── ui/                  # 前端：纯 HTML/CSS/JS，无构建步骤
├── icons/               # 应用图标（由 scripts/gen-icons.ps1 生成）
└── tauri.conf.json      # Tauri 配置
```

重新生成图标：`powershell -File scripts/gen-icons.ps1`

## 常见问题

- **复制了内容但界面不更新**：检查 `capabilities/default.json` 是否存在。
  Tauri 2 的权限系统会拒绝未授权的 plugin 命令（含 `event.listen`），
  导致后端在记录（`%APPDATA%\com.aentrance.clipboardhistory\history.json` 有数据）
  但界面收不到事件。前端诊断日志见同目录下 `ui.log`。
- **图片/文件不会出现**：本应用只记录文本内容，非文本内容会被跳过。
