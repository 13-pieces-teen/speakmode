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

前端使用 React 19、TypeScript、Vite 和 Radix，通过 typed `app_command` / events 连接 Rust。浏览器预览只展示界面，不生成示例数据，也不执行原生操作；自动化验证的模拟 bridge 仅存在于测试环境。原生后端负责操作权限、状态、资源、凭据及持久化，录音浮层的接口范围比主窗口更小。

## 会话、取消与提交

`engine` 绑定 session、generation 和 operation identity；处理旧会话的迟到回调不能更新新会话。停止采音、确认 ASR 尾段、整理、保存和输入有各自的状态与释放条件。

SQLite 由专用 worker 独占，使用有界队列、提交 guard 和最终 COMMIT 仲裁。Ready 的保存、使用量和当前版本一致提交；取消、删除、observer 中断与真正资源退出分别处理。历史编辑保留版本树，局部恢复与 undo 不改写原来源。

## Windows 输入

在停止意图进入会话时冻结窗口/进程/焦点身份，之后有界解析当前编辑位置。标准 Edit/RichEdit 使用类、样式、进程及选择元数据；其他控件通过 UIA 的 TextPattern 和明确可写证据识别。Group 富文本无 ValuePattern 时，仅接受严格布尔 false 的 IsReadOnly 属性。

派发前复核目标、caret、进程创建时间与修饰键；密码、只读、未知或更高权限目标不绕过检查。DeliveryAttempt 在输入前写入账本，已消费 permit 不会再次发送。SendInput 返回仅说明事件派发，产品状态保持 Unconfirmed；受控字段的测试读回不代表所有宿主都能确认。

## 云端和数据

`cloud` 使用 Rust WebSocket / reqwest，服务 host 由已验证的地域与业务空间生成。密钥经 Windows Credential Manager 获取，只在原生请求中使用。SourceRevision 与 OutputRevision 分别保留识别稿、输出 block 和来源引用，前端显示/复制同一成稿文本。

默认 debug 数据目录是项目内 `runtime/desktop`。显式 `SPEAKMODE_DATA_DIR` 必须是应用专用绝对目录。运行资料、数据库、音频、配置和临时测试数据都不提交 Git；分发包第三方材料保持独立冻结快照。

## 0.1.5 词典与采音

词典由 SQLite worker 持久化，主窗口 RPC 使用版本比较和提交 guard。一次录音开始时冻结词库，ASR 重试仍使用该版本。旧实时协议经 `session.update.input_audio_transcription.corpus.text` 发送；0.2.0的新Task Streaming接口按官方即时热词与有限context编码。两者都不做来源字符串替换，也不把词典作为Mode正文；录音浮层快照不包含完整词条。

CPAL 输入回调复用启动时预分配的 mono 缓存；非阻塞地收取空缓存、downmix、交给 worker。worker 重采样后返还缓存。2 秒原始样本预算、64 块上限、明确 overflow 故障、停止 cutoff 与 stream 销毁后的尾音排空继续生效；完整管线的其他线程仍可分配内存。具体增量与实测边界见[版本计划](plans/voice-0.1.5.md)。

## 0.2.0质量与纠词

`cloud/asr.rs`按兼容目录分派协议，`asr_task.rs`实现Audio3.1的run-task、ACK、二进制PCM、finish-task和最终任务回执。两条路径复用帧计数、取消和有界诊断；Task句号按sentence_id固定，partial可修订，只有全部final和任务完成才形成来源。模型变更只影响下次录音。

`revision.correctTerm`对显式选中的单处文本创建新输出版本，并可在同一数据库事务加入规范词条；绑定输出／词典版本与取消guard。MemoryOnly正文仍只在内存，用户明确记住的词独立落库。既有来源、局部恢复和undo契约不变，历史纠正不重复自动输入。

结构整理可以在一个连续sourceRange内归并话题，不能跨块任意重新挂来源。有限事实检查增加输出到有效来源的数值／单位／版本比对；不是通用语义判断。个人表达快捷选项写入现有个人Mode规则，规则仍不能提供口述中不存在的事实。见[升级计划](plans/voice-0.2.0.md)。
