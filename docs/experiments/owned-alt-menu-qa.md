# Right Alt、原生菜单与自动输入的独立验收

工具：`owned_alt_menu_probe` 与 `scripts/owned-native-editor.ps1`。目标是同一个长期运行的独立 Windows child PID，包含真实 Win32 菜单与标准 Edit 控件。只有已确认的合成控件允许输入和正文读回；不读取用户正文、资料目录、历史或凭据，不打开麦克风、不调用云服务、不关闭其他应用。

## 默认：合成菜单基线

由根统一构建当前源码的 example 后，在正常 Windows 环境运行。不要使用改变了 `USERPROFILE`／`APPDATA` 的 MSVC 构建环境执行验收。以下 `--out` 必须是未存在的绝对文件路径，scratch 与 output 父目录由本轮新建。

```powershell
& 'D:/Codex_Project/SpeakMode_Dev/cache/target-msvc-formal/x86_64-pc-windows-msvc/debug/examples/owned_alt_menu_probe.exe' `
  --fixture-script 'D:/Codex_Project/SpeakMode_Dev/publication/github-speakmode-2026-10-08/scripts/owned-native-editor.ps1' `
  --scratch-dir 'D:/Codex_Project/SpeakMode_Dev/evidence/voice-0.1.9-2026-10-08-r1/owned-alt-synthetic' `
  --out 'D:/Codex_Project/SpeakMode_Dev/evidence/voice-0.1.9-2026-10-08-r1/owned-alt-synthetic.json'
```

依次验证：

1. child 的 PID、存活、HWND、标题中的 UUID 和前台必须吻合；Edit 焦点与合成选区 `[6,7]` 必须成立。
2. 直接调用生产 `capture_foreground_anchor` → `capture_target_for_anchor` → `dispatch_once`，读回合成文本，验证 Unicode 插入及重复派发拒绝。
3. 仅向已确认前台的 child 发送带 `dwExtraInfo=0x534D414C` 标记的合成右 Alt 完整按下／释放；记录菜单请求、菜单循环、GUI menu flags、Edit 焦点，以及菜单状态下生产 capture 的接受／拒绝。
4. 在 child 内退出菜单并重置合成内容，再次执行生产 capture／dispatch 与读回。

合成模式**不启动 HotkeyService**。生产 hook 忽略所有 injected keyboard events，测试没有例外开关。因此这轮只能证明原生菜单基线与直接输入机制，**不能证明物理右 Alt 修复**。合成模式有效结束时为 `status=PARTIAL`、`mechanicsPassed=true`、`physicalPolicy.status=NOT_RUN`、`passed=false`；退出码 0 只表示已完成这轮限定的机制验证。菜单副作用未观察到时如实记 `menuSideEffectObserved=false`，不制造根因复现。

post-Alt 检查先取得不包含正文的 cheap anchor，再核对 child 的前台、root HWND、PID 与 Edit focus；菜单／其他焦点仅记录拒绝 metadata，`inputInspectionAttempted=false`，不会为找回目标而检查其他控件。失去 child ownership 或前台使用 typed precondition 标记，不能算产品捕获／派发失败；前置成立后生产 API 本身失败仍保留产品路径失败。

`afterFixtureReset` 是测试搭建操作后的验证；`fixtureResetIsProductionRecovery=false`。不能把测试窗重置理解为产品自动重获焦点。`coordinatorLedger=NOT_EXERCISED`：这个 example 验证单次 native permit 机制，不替代真实 coordinator 的 write-ahead Store／generation 账本回归。产品派发仍为 `Unconfirmed`，合成 fixture 独立读回成功不会改变该产品状态。

## 可选：物理右 Alt 验收

用户先从托盘自行退出 SpeakMode，并退出使用相同快捷键的其他听写工具，再在同一命令末尾添加 `--physical-alt`，使用不同 scratch／out 路径。工具只按进程名检查：任何 `SpeakMode.exe` 仍在运行，或进程清单无法确认，都会在创建 fixture／启动 hook 前拒绝；不会代替用户关闭软件。其他听写软件不能由本工具全面排除，因此用户应先退出，避免真实快捷键触发其他应用的录音。

前台成立且真实生产 HotkeyService 就绪后，fixture 中会显示操作提示。用户只在此合成文本框内实按两次右 Alt，两次相隔约两秒；这两次仅计数完整 Toggle，并核对 stop anchor／菜单／焦点，不创建录音 Session。第二次之后再调用生产 capture／dispatch，读取合成结果验证。

物理模式启动、等待手势期间前台必须一直是已确认的 child。每个手势最多等待 30 秒；失去前台、启动 hook 失败、正在运行的应用未退出、没有物理手势等前置问题为 `NOT_RUN`。已收到真正 Toggle 后菜单占据焦点或插入失败才归为产品路径失败。此模式直接使用公开 HotkeyService，不接受注入事件为物理事件，也不放宽 production filter。

receipt 保留 `observedPhysicalTapCount`：零实按时物理策略为 `NOT_RUN`；已经收到物理 Toggle 后的策略／派发失败为 `FAIL`，随后前置丢失或工具清理失败为 `PARTIAL`。hook startup 返回了资源 owner 时也必须取得 shutdown ACK；没有 ACK 则是 `physicalHookCleanup`／`toolFailure`，失败 owner 保留至测试进程退出，不以启动前置失败掩盖未释放资源。

## 当前证据边界

2026-10-08 编写工具时，仅执行 PowerShell AST 检查与 Windows PowerShell 的 C# `Add-Type` 编译检查，创建窗口数为零。Rust 由根统一编译；真实前台／菜单／插入实验结果必须引用单独 receipt，不能把工具存在视为验收通过。此前 0.1.7 native 8 场景都停在 `foregroundReadiness`，没有执行产品 capture／dispatch；当前工具仍只使用标准前台许可，拒绝则如实 `NOT_RUN`。
