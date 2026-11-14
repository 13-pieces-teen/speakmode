# SpeakMode 开发指南

当前桌面源码版本为 **0.2.41**，前端 package 版本为 **0.4.34**。快捷键重复唤起修复见[修复记录](experiments/hotkey-repeat-2026-11-14.md)；识别未完整后继续录音及恢复列表的逻辑见[恢复列表记录](experiments/recovery-next-recording-2026-11-13.md)。

## 环境准备

支持目标为 Windows x64，需要：

- Node.js 22.12+ 与 npm。
- 支持 edition 2024 的 Rust MSVC 工具链。
- Microsoft C++ Build Tools 与 Windows SDK。
- WebView2 Runtime。

仓库不包含个人配置、API Key、历史数据库、录音、安装包或本机工具链。第三方打包材料随仓库提供；保留 `app/` 与 `docs/third-party/nsis-resources/` 的相对布局。

## 本地运行

```powershell
git clone --branch fix/dictation-reliability-20261028 --single-branch https://github.com/13-pieces-teen/speakmode.git
cd speakmode/app
npm ci
npm run check
npm run build
npm run tauri -- dev --target x86_64-pc-windows-msvc
```

在应用 **设置 → 云端服务** 配置自己的服务、模型与 Key。新配置默认使用 `qwen3-asr-flash-realtime` 识别和 `qwen3.8-flash` 整理，可分别切换。具体可用性取决于账号、地域与供应商。

只预览前端时，可在 `app/` 运行 `npm run dev`。普通浏览器预览展示界面，不生成示例数据，也不执行原生录音、快捷键或跨应用输入。

日常开发与桌面检查应复用同一份绝对路径的 `CARGO_TARGET_DIR`，不要按版本复制编译缓存。详见[构建与缓存管理](build-cache.md)。

## 检查与打包

在 `app/` 检查前端：

```powershell
npm run check
npm run build
```

从仓库根目录运行前端合成 helper：

```powershell
node scripts/caption-helper-check.mjs
node scripts/waveform-helper-check.mjs
node scripts/live-level-check.mjs
node scripts/delivery-feedback-helper-check.mjs
node scripts/dictionary-helper-check.mjs
node scripts/restore-selection-helper-check.mjs
node scripts/correction-selection-helper-check.mjs
node scripts/term-correction-component-check.mjs
```

从仓库根目录运行 Windows 桌面检查、库测试和安装包构建：

```powershell
.\scripts\build-desktop.ps1 -Task Check -ValidateOnly
.\scripts\build-desktop.ps1 -Task Check
.\scripts\build-desktop.ps1 -Task Test
.\scripts\build-desktop.ps1 -Task Package
```

统一脚本使用 `x86_64-pc-windows-msvc` 和锁定依赖，在构建前后检查缓存与磁盘余量。打包调用本地 Tauri CLI，生成 Windows NSIS 安装包；不会启动应用或安装器。

合成 helper 不打开麦克风或请求云模型。Rust 测试使用项目专用 `.test-data/`；被忽略的 maintenance child 测试由父测试在受控目录启动。源码检查、真实口述、宿主输入与安装升级分别验收，具体记录见[验证文档](validation.md)。

## 代码导航

| 路径 | 内容 |
| --- | --- |
| `app/src/` | React 页面、组件、typed bridge、来源对照与录音浮层 |
| `app/src-tauri/src/engine/` | 会话状态、取消、场合、整理与一次性输入协调 |
| `app/src-tauri/src/platform/` | WASAPI 采音、重采样、全局快捷键与 UIA/Win32 输入 |
| `app/src-tauri/src/cloud/` | 供应商目录、ASR 协议、整理适配与系统凭据 |
| `app/src-tauri/src/core/` | SQLite worker、持久化、版本恢复与提交仲裁 |
| `app/src-tauri/windows/` | Windows NSIS 模板与维护 hooks |
| `docs/` | 产品、开发、验证与第三方材料 |
| `scripts/` | 构建入口与隔离验证工具 |

## README 图片

当前界面截图从生产构建生成，固定示例数据仅由隔离浏览器 bridge 注入：

```powershell
npm --prefix app run build
python scripts/capture-readme-assets.py
```

该工具需要已有的 Python Playwright 和 Microsoft Edge；截图输出至 `docs/assets/readme/`，采集收据写入被忽略的 `.validation/readme-assets/`。制作与素材分类见[README 素材说明](readme-assets.md)。
