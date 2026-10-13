# 输入 Helper 调查与宿主验证：PR #8 之后的 B1

本轮完成了原生分析环境、四个目标导出的调用链调查，以及受控输入验收工具扩展。Windows 原生 16/16、隔离 Edge 19/19 通过；Word 在就绪文档中曾连续四轮各 5 个场景通过，但保留一次早期 UIA 超时，末两轮又因 Windows 不授予前台权限而在输入前停止。**B1 仍为 PARTIAL，不能宣称全部宿主验收完成。**

这次不改生产输入策略、不加固定等待、不恢复剪贴板、不自动补发，也不更改版本号或生成新安装包。提交的实现是可复现的真实宿主测试和调查证据。生产代码仍来自 [PR #8](https://github.com/13-pieces-teen/speakmode/pull/8) 的 `4e3db04`，`platform/delivery.rs` 与 `platform/mod.rs` 均未改动。文件 ASR 的 `DO_NOT_PROMOTE` 决定延续；本轮新增 ASR 请求为零。

## 1. REA 原生调查已解除环境阻塞

使用 REA 6.4.0、Ghidra 12.1.4、Temurin 64 位完整 JDK 21.0.12.1。工具链下载到本机项目 `toolchains/rea-native`，归档 SHA-256 与官方发布元数据一致；只对子进程设置 `GHIDRA_INSTALL_DIR`、`JAVA_HOME`，未改变用户或系统设置。

- Ghidra：`ddac49f903da9d5bac833e5cc79395098b9c33cfd3279be5f31bd00387d2d4db`。[官方发行](https://github.com/NationalSecurityAgency/ghidra/releases/tag/Ghidra_12.1.4_build)
- JDK：`f9d6e191ab098c0d416e7d588a24420a8621cd2f4720dab2459b8b7b2d2d8b4e`。[官方发行](https://github.com/adoptium/temurin21-binaries/releases/tag/jdk-21.0.12.1%2B1)
- 目标 `InputHelper.dll`：`897585ae414e5a83a4a0aa649447291bb4711909b4460800674e91a418ed54d5`，x86-64 PE。REA 返回 1,221 个过程、705 个字符串及 `ephemeral-source-immutable` 导入模式。全程未加载执行该 DLL，也未运行 Typeless 动态插入。

`doctor` 的 Ghidra、JDK、Windows Job Object / 私有 DACL / NTFS 检查通过。先 `inspect`，再对四个导出及 11 个相关过程执行 `function`，共保存 15 份带证据 ID 的函数结果。Ghidra 对导出报告为 thunk；以下明确区分导出入口、实现和下游函数。伪代码仍有未还原的类型与间接调用，不能作为原始源码或运行实测。

| 调查对象 | 原生证据 | 可以支持的结论 |
| --- | --- | --- |
| `insertText`，入口 `0x18000f480` → `0x18000df30` | 备份剪贴板格式；设置文本；调用 `0x18000bbe0`；启动 `0x18000c3b0` 恢复线程 | 普通插入采用剪贴板与键盘派发路径；并非宿主文本回读 |
| 模拟粘贴，`0x18000bbe0` | Ctrl 按下、V 按下/抬起、Ctrl 抬起；`SendInput(4, …)`；比较返回值是否等于 4 | 返回 1 的条件是四个事件进入输入流。它不能证明目标文字已插入 |
| 普通恢复，`0x18000c3b0` → `0x18000cac0` | 等待 500 ms；最多 5 次尝试打开剪贴板，失败间隔 100 ms；清空并逐格式恢复 | 有异步恢复机制。已检查函数未见剪贴板序号/内容所有权校验，不能据此保证不覆盖期间的新复制 |
| `insertRichText`，入口 `0x18000f470` → `0x18000cea0` | `OleSetClipboard`、`OleFlushClipboard`、相同模拟粘贴函数；恢复线程 `0x18000c2d0` 等待 1,000 ms | 富文本有不同的 OLE 数据路径与恢复时序；未验证宿主格式兼容性 |
| `getCurrentInputState`，入口 `0x18000f440` → `0x18000f6c0` | 经 `0x1800165b0` 创建/缓存 CUIAutomation8；`0x180014e10` 生成选区起止和前后文本字段 | 原生确有 UIA 输入状态读取，不只是 JS 壳；不能据此推定所有宿主都成功 |
| UIA 配置，`0x180011970` | IID `34723aff-0c9d-49d0-9896-7ab52df8cd8a`；vtable `0x1e8` / `0x1f8` 设置 5000 / 10000 | 与本机 Windows bindings 核对，分别为连接和事务超时；这是预算，不是实际耗时 |
| `getSelectedTextBySimulateCopyAsync`，入口 `0x18000f460` → `0x180007ca0` | 工作线程 `0x180002580` → `0x180004ce0`；清空剪贴板后等 50 ms；本地读取分支失败后发 Ctrl+C、等 200 ms、读出并恢复文本 | 可见模拟复制后备及副作用；本地读取分支内部、调用期间焦点稳定性和取消竞态未完整证明 |

上述等待单位由 `0x180002b00` 的毫秒到纳秒换算及 `Sleep` 调用核实。UIA vtable 偏移与 `windows` 0.58.0 的 `IUIAutomation2_Vtbl` 对应，不能把配置中的 5/10 秒当作 Typeless 的实际端到端等待。[连接超时](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation2-put_connectiontimeout)、[事务超时](https://learn.microsoft.com/en-us/windows/win32/api/uiautomationclient/nf-uiautomationclient-iuiautomation2-put_transactiontimeout)

Microsoft 对 `SendInput` 返回值的定义同样是进入输入流的事件数。因此继续使用 SpeakMode 的 `Unconfirmed` 回执是有依据的；现有 JS 500 ms 等待也不构成宿主确认。[SendInput 返回语义](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)

若后续设计剪贴板恢复，至少要在持有剪贴板锁时重新检查本次写入的序号/所有权，覆盖用户再次复制、连续两次听写、慢宿主与取消等竞态；序号相同也不能证明宿主已消费文本。当前证据不支持直接照搬固定延时恢复。[剪贴板序号](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getclipboardsequencenumber)

## 2. 已实现的受控宿主测试

所有输入均为固定合成文本。探针调用生产 `capture_target_for_anchor` / `dispatch_once`，并检查重复 permit 无法再次输入。回读只发生于独立创建并验证 PID、创建时间、HWND、UUID 标识的测试宿主；生产回执仍为 `Unconfirmed`。不读取个人文档、密码字段或浏览器资料。

- [原生探针](../../app/src-tauri/examples/owned_delivery_probe.rs)与[原生宿主](../../scripts/owned-native-editor.ps1)：8 → **16** 场景。新增 Edit/RichEdit 的 emoji、40 行文本、派发前取消及切换至另一个自有顶层窗口。原有选区替换、只读/密码拒绝、光标/字段变化拒绝继续验证。
- [浏览器探针](../../app/src-tauri/examples/owned_browser_delivery_probe.rs)与[Edge 驱动](../../scripts/owned-browser-delivery-qa.py)：9 → **19** 场景。textarea、contenteditable 各新增五场景；新建第二个真实顶层窗口，确认换窗后两个字段均未收到输入。使用独立浏览器资料与本地文件，结束后验证自有进程已清理。
- [Word 探针](../../app/src-tauri/examples/owned_word_delivery_probe.rs)与[Word 驱动](../../scripts/owned-word-delivery-qa.py)：创建新的 Word COM 进程及两个 UUID 文档，验证五场景，直接回读自有 Word 文档。准备阶段检查布局、屏幕刷新和 COM 选区；正常前台权限交接失败时停止，不模拟 Alt 或绕过 Windows 焦点规则。
- [共享合成内容](../../app/src-tauri/examples/support/owned_delivery_cases.rs)：中文、English、组合 emoji `👩🏽‍💻`、火箭和组合附加符；长文本 40 行、2,038 个 UTF-16 单元。长文本验收逐字符比较，不只比较长度。

换窗与取消发生在捕获目标之后、原生派发之前。这些测试不包含真实麦克风、完整 ASR/整理等待或用户实际操作，因此不能替代 B2 的完整会话测量。

| 运行 | 输入场景结果 | 运行与清理状态 |
| --- | --- | --- |
| 原生 | 16/16 | PASS |
| Edge | 19/19 | PASS；源码/二进制一致、自有进程清理完成 |
| Word 最初启动 | 0 个输入场景 | 测试脚本误取 Application.Hwnd；改用 Window.Hwnd 并核对新进程；失败实例已按 PID/创建时间清理 |
| Word r2 | 4/5 | 选区替换在 UIA 捕获阶段超时；0 输入事件、未改剪贴板。记录为产品路径异常，不丢弃 |
| Word r3–r5 | 各 5/5 | 文档就绪后通过；但 COM Quit 后留下无窗口后台进程，运行总体未记为 PASS。随后核验并清理 |
| Word r6 | 5/5 | PASS；关闭自有文档、Quit 返回后，仅对同一创建时间且没有可见窗口的残留进程完成清理 |
| Word final / final-r2 | 各 5 个场景均未进入输入 | Windows 拒绝前台切换，最终 `AllowSetForegroundWindow=false`；记录为测试前提失败，清理完成 |
| Word 无效二进制预检 | 0 个输入场景 | 在启动 Word 和创建测试目录前拒绝；保留失败收据 |

Word 实际进入产品路径的 25 个场景中 24 个通过、1 个 UIA 超时；另有 10 个场景因前台前提不成立而未进入产品路径。脚本调整、机器负载和时间顺序均有变化，**不能把后续通过归因为 SpeakMode 已修复超时**。生产 UIA 仍为 200 ms 单项超时和 1 秒外层上限；未借竞品的 5/10 秒设置放宽它。

原计划 30 格矩阵现为 **15 PASS、5 Word CONDITIONAL、10 NOT_RUN**。Word 保留条件状态，飞书/VS Code 没有本轮验证过的独立宿主。详见[矩阵](input-helper-host-matrix-2026-10-13.csv)及[可审查的证据索引](input-helper-evidence-2026-10-13.json)。没有把普通控件成功外推为所有应用成功。

## 3. 复现与后续条件

在已配置的 MSVC 环境中先运行仓库构建预检，并复用现有 target；然后构建三个 desktop examples：

```powershell
.\scripts\build-desktop.ps1 -Task Check -ValidateOnly -TargetDirectory $env:CARGO_TARGET_DIR
cargo build --locked --manifest-path app/src-tauri/Cargo.toml --target x86_64-pc-windows-msvc --features desktop --example owned_delivery_probe --example owned_browser_delivery_probe --example owned_word_delivery_probe
```

各测试要求显式绝对路径和不存在的输出文件。原生探针接受 `--fixture-script`、`--scratch-dir`、`--out`；Edge 驱动接受 `--binary`、`--edge`、`--scratch-dir`、`--out`；Word 驱动接受 `--binary`、`--scratch-dir`、`--out`，另需本机 Word、psutil、pywin32。宿主测试须能正常取得前台；权限不成立的运行保留为前提失败，不改为通过。

本机原始证据在 `D:/Codex_Project/SpeakMode_Dev/output/validation/input-helper-2026-10-13`，包括所有成功和失败收据、15 份 REA 函数证据、工具链校验及构建日志。仓库仅收录哈希、地址、摘要和自有测试代码，不收录厂商 DLL、反编译正文、Office 文件或浏览器资料。Rust 三个探针构建通过，Python 语法检查和 `git diff --check` 通过；生产库未修改，不将上一轮 743 项结果称为本轮重新执行。

下一步 B1 要先在正常前台会话中完成 Word 稳定性重复测量，给超时定位到具体 UIA 阶段，再判断是否需要最小适配；飞书和 VS Code 需要各自独立宿主。B2 仍缺核对真人音频和本轮 Typeless 动态观测。C 仍缺质量达标的独立预留集，这轮调查没有改变文件恢复暂不推广的决定。
