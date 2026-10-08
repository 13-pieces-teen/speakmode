# 技术结构

## 主要路径

```text
右 Alt / 浮层操作
  → Tauri desktop coordinator / 会话 generation
  → CPAL WASAPI 采音 → FrameResampler → 有界音频队列
  → 百炼 ASR WebSocket → typed source items / 流式字幕
  → final source → Mode prompt + HTTP 整理 → output blocks / source refs
  → SQLite worker 提交 Ready / 使用量
  → DeliveryAttempt 账本 → Windows 目标复核 → 一次 Ctrl+V 派发
```

前端使用 React 19、TypeScript、Vite 和 Radix，通过 typed `app_command` / events 连接 Rust。浏览器预览使用独立 mock bridge。原生后端负责操作权限、状态、资源、凭据及持久化，录音浮层的接口范围比主窗口更小。

## 会话、取消与提交

`engine` 绑定 session、generation 和 operation identity；处理旧会话的迟到回调不能更新新会话。停止采音、确认 ASR 尾段、整理、保存和输入有各自的状态与释放条件。

SQLite 由专用 worker 独占，使用有界队列、提交 guard 和最终 COMMIT 仲裁。Ready 的保存、使用量和当前版本一致提交；取消、删除、observer 中断与真正资源退出分别处理。历史编辑保留版本树，局部恢复与 undo 不改写原来源。

## Windows 输入

在停止意图进入会话时冻结窗口/进程/焦点身份，之后有界解析当前编辑位置。标准 Edit/RichEdit 使用类、样式、进程及选择元数据；其他控件通过 UIA 的 TextPattern 和明确可写证据识别。Group 富文本无 ValuePattern 时，仅接受严格布尔 false 的 IsReadOnly 属性。

派发前复核目标、caret、进程创建时间与修饰键；密码、只读、未知或更高权限目标不绕过检查。DeliveryAttempt 在输入前写入账本，已消费 permit 不会再次发送。SendInput 返回仅说明事件派发，产品状态保持 Unconfirmed；受控字段的测试读回不代表所有宿主都能确认。

## 云端和数据

`cloud` 使用 Rust WebSocket / reqwest，服务 host 由已验证的地域与业务空间生成。密钥经 Windows Credential Manager 获取，只在原生请求中使用。SourceRevision 与 OutputRevision 分别保留识别稿、输出 block 和来源引用，前端显示/复制同一成稿文本。

默认 debug 数据目录是项目内 `runtime/desktop`。显式 `SPEAKMODE_DATA_DIR` 必须是应用专用绝对目录。运行资料、数据库、音频、配置和临时测试数据都不提交 Git；分发包第三方材料保持独立冻结快照。
