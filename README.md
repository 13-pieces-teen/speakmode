# SpeakMode

SpeakMode 是一个 Windows 语音输入 MVP：按右 Alt 开始口述，再按一次结束，将识别结果按所选 Mode 整理后输入到结束口述时的文本位置。录音中可以停顿、翻阅资料或移动光标。

当前桌面源码版本为 **0.1.5**，前端 package 版本为 **0.3.1**。仓库包含实际 React/Tauri/Rust 实现、锁定依赖、测试和构建所需的第三方材料；不包含个人配置、API Key、历史数据库、录音、安装包或本机工具链。

## 功能

- 全局录音快捷键、Windows 原生悬浮栏、真实音量反馈和流式字幕。
- Auto、轻整理、结构与个人 Mode；个人规则控制格式，不能覆盖来源和输出契约。
- 云端语音识别与文字整理，目前支持阿里云百炼北京/新加坡地域。
- 个人词典支持新增、编辑、删除、搜索及批量添加，作为下一次录音的 ASR 词汇上下文。
- 历史记录、今日使用量、识别稿对照、局部恢复及撤销。
- 整理后尝试一次自动输入；目标改变或无法验证时保留成稿并明确提示，支持手动复制。
- 快捷键、麦克风、语言、默认 Mode、输入方式、历史保存与云端配置设置。

## 开发

支持目标为 Windows x64。需要 Node.js **22.12+**、npm、Rust MSVC 工具链、Microsoft C++ Build Tools / Windows SDK 和 WebView2 Runtime。Rust crate 使用 edition 2024；本机原始验证使用 Rust 1.99.0。此仓库不安装或分发上述开发工具。

```powershell
git clone https://github.com/13-pieces-teen/speakmode.git
cd speakmode/app
npm ci
npm run check
npm run build
npm run tauri -- dev --target x86_64-pc-windows-msvc
```

在应用“设置 → 云端服务”中配置自己的 Key 和所属地域。默认模型为 `qwen3-asr-flash-realtime` 和 `qwen-flash`；模型可用性、额度和地域取决于你的账号。Key 通过 Windows Credential Manager 保存，不写入 Git 文件或发送到前端日志。

浏览器单独运行 `npm run dev` 可检查界面，但不生成示例数据或执行原生操作，不能代替桌面语音、系统快捷键或跨应用输入验收。

## 验证和打包

从仓库根目录运行前端合成 helper 检查：

```powershell
node scripts/caption-helper-check.mjs
node scripts/waveform-helper-check.mjs
node scripts/live-level-check.mjs
node scripts/delivery-feedback-helper-check.mjs
node scripts/dictionary-helper-check.mjs
```

从 `app/` 运行 Windows 桌面库测试和安装包构建：

```powershell
cargo test --locked --manifest-path src-tauri/Cargo.toml --target x86_64-pc-windows-msvc --features desktop --lib -- --test-threads=4
npm.cmd run desktop:build -- --ci -- --locked
```

合成 helper 不打开麦克风或请求云模型；Rust 测试使用项目专有 `.test-data/`。不要直接运行被忽略的 maintenance child 测试，它由父测试在受控目录启动。

PowerShell 中带 `--` 分隔的打包命令使用 `npm.cmd`，避免 `npm.ps1` 吞掉参数分隔符。

验证状态与实际验收边界见 [docs/validation.md](docs/validation.md)。安装包构建、真实口述、实际宿主输入及安装/升级/卸载是不同的验证范围，不把源码测试等同于整体产品验收。

0.1.5 的词典规则、采音改动、测量口径与实际口述对比见 [语音增量计划](docs/plans/voice-0.1.5.md)。

固定 TTS 的真实云端对比、默认麦克风采音与自有 Windows 控件实验见 [真实语音实验报告](docs/experiments/voice-real-lab-2026-10-08.md)。目前已观察到 Mode 来源映射失败与时间限定遗漏；实验通过的范围和未完成的用户验收分别记录。`app/src-tauri/examples/` 中的实验工具须显式运行，正常程序启动不执行实验。

## 目录

| 路径 | 内容 |
| --- | --- |
| `app/src/` | React 页面、组件、typed bridge、来源对照和录音浮层 |
| `app/src-tauri/src/engine/` | 会话状态、取消、Mode、整理和一次性输入协调 |
| `app/src-tauri/src/platform/` | WASAPI 采音、重采样、全局快捷键、UIA/Win32 输入 |
| `app/src-tauri/src/cloud/` | 百炼 ASR WebSocket、整理 HTTP 和系统凭据 |
| `app/src-tauri/src/core/` | SQLite worker、持久化、版本恢复和提交仲裁 |
| `app/src-tauri/windows/` | Windows NSIS 模板和维护 hooks |
| `docs/` | MVP、架构、验证及第三方材料 |
| `scripts/` | 可移植的前端合成 helper 检查 |

保留 `app/` 与 `docs/third-party/nsis-resources/` 的相对布局；Tauri 打包需要该目录中的 14 个文件。第三方材料与已知缺口见 [docs/third-party/README.md](docs/third-party/README.md)。项目未新增第一方开源授权，第三方代码和资源遵循各自随附许可证。
