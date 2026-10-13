# 录音浮层 Mode 0.2.7：验证记录

日期：2026-10-09。桌面 0.2.7，前端 0.4.6；基线为本机 0.2.6。工作树尚未提交／推送。

## 实际调整

透明根因是 `html[data-surface=recorder] .radix-themes` 同时匹配 Popover 本身，且其 `!important` 优先级超过菜单白底。透明规则现仅命中文档、body 与 root Theme。菜单本身始终白底、opacity 1，窗口周围继续透明。

菜单从 280px 宽、61px 行高调整为 216px 宽、36px 行高；基础三项菜单实测 152px 高。常驻说明、大图标底座和重复页脚移除；标题“本次 Mode”、图标、名称及勾选保留，说明和完整名称可悬停查看。个人 Mode 长名称省略，选项区域单独滚动。

鼠标展开为 150ms transform 小位移／缩放，无 opacity 淡入；键盘、托盘及 reduced motion 即时显示。关闭不延迟原生 `menuOpen:false` 请求。菜单先在隐藏状态测量，窗口高度和实际定位就绪后显示，避免初始小窗口顶部露出半截。字幕展开、错误、录音限时提示与托盘说明组合时，临时收紧字幕区；关闭后恢复原阅读状态。

## 测试迭代

全部浏览器用例使用真实生产 React 和隔离合成 IPC；模拟原生 resize 请求的 viewport 调整及串行回执，不访问个人资料或麦克风。

- r1：基础 15 项通过；逐帧观察随后发现原 86px 窗口展开时的短暂裁切。
- r2：增加托盘＋多行错误＋展开字幕＋录音限时＋24 个个人 Mode，15/16；选项区被挤成 0px，保留失败证据。
- r3：压缩辅助内容，选项恢复可见；15/16，reduced motion 暴露窗口尺寸先完成、Radix 定位晚一帧的问题。
- r4：增加真实位置门槛，15/16；定位变化观察绑定过早，reduced motion 菜单保持隐藏。改为测量时绑定，原生尺寸回执后复测，最多 12 帧补偿定位就绪，避免长期轮询。
- r5：原 16 项通过；新增动态字幕／错误的键盘焦点用例失败（16/17）。隐藏重定位期间焦点丢失，现记录并恢复正在浏览的选项，而不是跳回当前已选 Mode。

## 最终浏览器结果

r6 **17/17 PASS**，Edge 155.0.4283.45；页面 JS 异常和外部请求均为 0。相同合成 resize 通道最大并发为 1，关闭后发送 `menuOpen:false`，选择包含会话 ID、generation 和 Mode revision。

- 基础三项：菜单 216×152，窗口 440×248；实色白底、opacity 1，选项 36px 高。
- 最坏组合：窗口 440×420，菜单 216×120；选项区域仍有 44px，End 定位的最后一项 36px 完整可见。
- 动态字幕和错误出现：正在浏览的个人 Mode 保持焦点和可见性。
- 逐帧捕获确认 pointer 有实际 transform 过渡，keyboard／tray／reduced motion 无过渡；所有可见帧完整位于窗口内，菜单没有 opacity 淡入。
- 6 次快速开关后最终状态正确；Escape 和失焦关闭菜单不发送 stop/cancel。
- 既有恢复／交付浏览器回归最终 **11/11 PASS**，共 28 项不同用例。restore 首次调用漏传 `--serve` 导致连接拒绝、0 项执行；补齐受控预览服务后通过，未把该工具错误计作产品缺陷。

前端 TypeScript／Vite 构建通过；仍有项目已有的 Radix `use client` 和 chunk 大小提示。最终检查脚本 `scripts/recorder-mode-browser-qa.py` 可用 `--help` 复现，证据目录 r6；恢复回归目录后缀 `-restore-final`。

## Windows 构建与交付

统一 MSVC `scripts/build-desktop.ps1 -Task Package` 退出 0，release 编译日志显示 2m44s，NSIS 安装包生成成功。复用 `cache/target-msvc-formal`，构建后 4.930 GiB，增长 99,930,454 bytes；D 盘余量 61.251 GiB，未越预算。没有新增按版本划分的编译缓存。

188 项 app／第三方构建输入在构建前后 SHA256 一致。EXE／安装包 ProductVersion 和 FileVersion 均为 0.2.7，复制后的 SHA 一致；13 件清单资源及清单副本核对通过。旧第三方清单中的历史源码哈希不作为本版构建哈希，当前输入另外留有前后收据。

| 文件 | Bytes | SHA256 |
| --- | ---: | --- |
| SpeakMode.exe | 18,202,624 | `677ef056eb2977c29dc0b66466c9b7c69cdcfc72f608bc1b7fd0e44916fbbbb9` |
| SpeakMode_0.2.7_x64-setup.exe | 5,600,661 | `289812bf89722eb1c4c31ed5561de65796adb0ae047c1c694792ec38dbae87b7` |

本机交付目录：`D:/Codex_Project/SpeakMode_Dev/output/releases/2026-10-09-recorder-mode-0.2.7`。含 EXE、NSIS、`Start-SpeakMode.cmd`／`.ps1`、third-party 和 `build-receipt.json`。启动器 PowerShell 语法通过，版本与 EXE hash 固定，继续使用原资料目录 `runtime/native-qa-2026-10-04`，检测旧进程而不会强行终止。应用、启动器与安装器未自动执行。

## 验证范围

本版没有修改 Rust 功能、采音、云端模型、整理规则、数据库、原生目标捕获或焦点恢复策略，不重复引用历史 Rust 测试数作为本版成绩。浏览器 viewport 模拟不等于真实 Win32/WebView2 透明合成、多屏 DPI、物理快捷键与跨应用输入验收。应用与安装器不自动启动；真实 Windows 操作仍需用户复测。

本机证据前缀：`D:/Codex_Project/SpeakMode_Dev/evidence/recorder-mode-0.2.7-2026-10-09`，各轮使用独立目录保留失败。
