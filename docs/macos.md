# macOS 开发与验证

Mac 与 Windows 使用同一份前端、云端协议、整理规则、数据模型和交付状态机。
Mac 原生边界负责麦克风、快捷键、输入位置、剪贴板、窗口、权限、钥匙串和退出资源回收。
默认快捷键在 Mac 显示为右 Option，也可在设置录制 Command / Control / Option 组合键。

## 构建

需要 macOS、Xcode Command Line Tools、Node.js 22.12+、npm 和支持 edition 2024 的 Rust。
在仓库根目录执行：

```sh
npm --prefix app ci
./scripts/build-macos.sh Check
./scripts/build-macos.sh Test
./scripts/build-macos.sh Package
```

脚本默认设置 `MACOSX_DEPLOYMENT_TARGET=12.0`，复用 `app/src-tauri/target`。
Tauri 自动合并 `tauri.macos.conf.json`，生成 `.app` 和 `.dmg`。
可把 Rust target 作为脚本第二个参数；先自行安装对应的 Rust 标准库。
Apple Silicon 原生使用 `aarch64-apple-darwin`，Intel 使用 `x86_64-apple-darwin`；
通用包可用 `Package universal-apple-darwin`，需要同时安装两种 target。
最低系统版本设为 macOS 12，此前 0.2.30 的原生验收使用 Intel / macOS 26.6.2。旧系统和 Apple Silicon 的验收仍待完成。

本地调试包：

```sh
cd app
npm run tauri -- build --debug --bundles app
```

发布包位于 `app/src-tauri/target/release/bundle/`，调试包位于 `target/debug/bundle/`。
默认使用 ad-hoc 本地签名，不需要私钥。公开分发前应通过 `APPLE_SIGNING_IDENTITY`
配置自己的 Apple Developer ID，并配置公证凭据；仓库不包含任何签名私钥。
ad-hoc 构建的代码签名随二进制变化，重新构建后可能需要重新授权。开发期间固定主应用路径，
把原生测试窗口作为独立程序编译，可避免调整测试工具时反复改变主应用的授权身份。
签名与隐私权限的对应机制见 [Apple TN3127](https://developer.apple.com/documentation/technotes/tn3127-inside-code-signing-requirements/)。

## 权限与使用

首次使用点击继续进入准备页面，再按提示授权麦克风和辅助功能。
系统的辅助功能开关必须由用户本人批准。列表缺少 SpeakMode 时，使用“＋”添加正在测试的 `SpeakMode.app`。
在文件选择窗口按 `⌘⇧G`，输入应用所在的 `target/debug/bundle/macos` 或 `target/release/bundle/macos` 完整目录即可定位。
需要时再按界面提示检查输入监控；返回应用后自动检查权限并恢复快捷键，也可手动重新启用。
撤销授权、锁屏、休眠或安全输入会中止当前快捷键手势并关闭录音准入，恢复后重新建立状态。

本地验收曾遇到开关已开但系统仍要求旧代码签名的情况。先退出应用，移除旧条目并添加固定的当前包。
仍不匹配时可执行 `tccutil reset Accessibility dev.speakmode.desktop`，只清除本应用的辅助功能记录，
随后重新发起请求并由用户开启。不要在调试包与发行包之间反复切换授权；本地 ad-hoc 构建的签名随代码变化。

密码框、安全输入、只读控件、未知可编辑性，以及停止后切换输入框或移动选区，均不会强行粘贴。
文本仍保留在历史/恢复入口供复制。可选的输入位置感知只读取有界邻近文本，沿用已有默认关闭设置。
正常输入使用一次带来源标记的 Cmd+V；收据只表示已派发，不把键盘事件计数当作插入成功。

Key 使用登录钥匙串；正式数据默认在 `~/Library/Application Support/dev.speakmode.desktop/`。
调试数据默认位于项目 `runtime/desktop`，不复用正式数据。退出、开机启动、菜单栏和停止时目标校验复用原协调器。
Typeless 导入只在用户点击时读取本机固定缓存目录，沿用完整性检查；尚未取得真实 Mac Typeless 缓存验收。

## 验证与边界

### 0.2.35 分支同步（2026-11-06）

`macos-parity` 合入主分支 `101f6f7`，包含录音启动状态修复 `00b279f`。
桌面版本更新为 0.2.35，前端版本更新为 0.4.32。

- 麦克风收到实际采样后显示“正在聆听”和声波。录音字幕偏好支持保存、读取和旧配置默认值。
- 原生浮层尺寸同步为 298 × 68，并接入按会话校验的关闭操作。
- Mac 投递实现适配新版共享接口，继续使用 AX 输入框身份和选区校验。
- Tauri 的 `macOSPrivateApi` 移至共享配置，与 Cargo feature 对齐。平台打包配置仍分别使用 `.app` / `.dmg` 和 NSIS。

本次在 Windows x64 完成 TypeScript / Vite 构建、13 个前端辅助/组件检查入口、
Mac / Windows 快捷键与字幕偏好回归，以及桌面 Rust 测试，共 1087 项通过、0 失败、4 项原有忽略。
构建仍有 Radix 的 `use client` 和产物体积提示。
0.2.35 的 macOS 原生编译、安装包、物理快捷键、真实录音转写和跨应用输入均待 Mac 环境验收。

### 0.2.30 原生验证记录

此前在 Mac 完成前端 TypeScript / Vite 构建、12 个前端辅助/组件检查入口，以及 914 项桌面 Rust 回归
（0 失败、3 项原有忽略）。键盘库的 57 项单测通过，其中 3 个实际 event-tap 用例在测试进程没有授权时提前跳过，
不能据此称为真实物理快捷键验收。新增的粘贴来源过滤与重置世代回调测试实际执行并通过。
另在已授权的同一应用包中完成真实 CoreAudio 采样、钥匙串写入→读取→删除，
以及原生 Cocoa 文本框的中文/emoji 粘贴、有界 Unicode 上下文读取、一次性派发、
取消与选区变化拒绝、只读和密码框拒绝测试。
采样测试不保存声音文件、不请求云端；钥匙串测试使用随机测试项并验证删除。

用户测试曾在点击辅助功能按钮时崩溃，完整报告指向 `AXIsProcessTrustedWithOptions`。
原实现使用无键值回调的 CFDictionary，并传入新建 NSString 作为 key，系统无法按预期查找。
修复使用 Apple 导出的 key 与 Foundation 管理的字典，并将授权操作送回主线程；
增加不弹窗的原生回归测试，覆盖同一 API 的字典构造和释放。

以下仍需真实验收，不能由上述本机验收代替：全局物理快捷键、多显示器/全屏浮层、
真实账号的完整口述链路、Apple Silicon、macOS 12 及签名公证分发。
Windows 构建使用 `scripts/build-desktop.ps1`，NSIS 配置由 `tauri.windows.conf.json` 自动合并。

调试构建提供显式原生探针，发行构建不包含探针：

```sh
app/src-tauri/target/debug/bundle/macos/SpeakMode.app/Contents/MacOS/SpeakMode --native-probe
# 按需单独使用 --audio、--keychain、--request-accessibility 或 --delivery。
```

`--request-accessibility` 复用生产授权调用，只发起系统提示和打开设置，不批准授权。
`--delivery` 需要辅助功能权限，使用独立的自有 Cocoa 文本控件验证 Unicode 粘贴、
一次性派发、取消、选区变化、只读和密码框拒绝；测试前备份剪贴板，仅在没有外部变更时恢复。
可先用 `cargo build --manifest-path app/src-tauri/Cargo.toml --features desktop --example macos_native_probe`
单独构建测试窗口，再给已授权应用的 `--delivery` 传入该可执行文件的绝对路径。
这样修改窗口时只需重编 example，保留主应用的签名和授权。
探针不读取用户文档或业务凭据，不记录音频，不调用云服务。
