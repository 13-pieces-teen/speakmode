# SpeakMode 0.1.7：恢复、成稿找回与缓存重试

日期：2026-10-08。比较基线为 0.1.6 产品候选 `779fa56ea3acb6bc923c552b1eb9269370dc9cd0`，本轮从仓库 HEAD `980f5c4f4790e945e54bb3feac0780b709cd1053` 实施。桌面版本 0.1.7，前端 0.3.2。范围、增量与剩余条目见[版本计划](../plans/voice-0.1.7.md)和[优化清单](../plans/optimization-backlog-2026-10-08.md)。

## 可交付增量

| 项目 | 之前 → 现在 | 已证实的结果 |
| --- | --- | --- |
| 精确恢复 | 点击光标仍可能沿用旧选区 → 明确记录插入／替换意图，不默认替换整个结果段 | 实际 Store 支持 emoji 后插入“不要”，其余文字与来源保持、撤销可还原；生产 React 的“今天”插入、提交和撤销通过 |
| 找回成稿 | Unconfirmed 浮层无入口 → “查看成稿”打开主面板 | 合成 bridge 验证只调用 `app.showMain`，不隐式复制或重发；正在派发不增加竞争操作 |
| 权限原因 | 更高完整性目标被归为不可用 → 保留 `targetHigherIntegrity`，解释需手动粘贴 | anchor、inspection、Mode 返回、迟到 generation 和一次 Skipped 账本回归通过；没有管理员宿主真实验收 |
| 识别诊断 | 仅队列峰值和阶段时间 → 采音／入队／出队／实际 socket 写出、Finish／终止、pending／final 和有界事件类型 | 固定音频实际写出计数和终态元数据可核对；不将 socket 写出当作服务端逐帧 ACK |
| 缓存重试 | 直接突发回放 → 等配置 ACK 后按实际音频时长分帧发送，不积攒补发额度 | 两条固定 PCM 的候选重试均完整成稿、关键内容复核通过；代价是重试等待约音频长度 |

没有改动正常实时录音的节拍、模型、地域、VAD、麦克风参数、快捷键、来源覆盖检查或单次派发契约。没有证明一般语义准确率、正常输入速度或日常正确粘贴率提升。

## 工程回归与浏览器

- Windows desktop 全库最终 **367 passed、0 failed、3 ignored**；examples 7 项通过，examples 构建完成。较 0.1.6 默认测试增加 18 项，不累加中间复跑。156 个源码／工具输入前后相同。
- 六个前端 helper **56 项通过**；TypeScript 与 Vite 通过。既有 Radix directive、chunk size 和 Rust dead-code 警告保留。
- 诊断导出器支持 schema1/2，**21 项 mock 边界测试通过**：有界事件、合法错误码、采样率口径、过滤正文／PCM／Key／原始 item ID。没有对正在运行的用户应用执行本轮真实 RPC 导出验收。
- 隔离 headless Edge 生产 React／合成 Tauri bridge 最终 **11/11 通过**，无页面错误或外部请求。包含真实鼠标插入“今天”→scalar `[1,1]`→确认→页面更新→撤销，以及来源收起、跨段／版本变更、对照关闭重开时清除旧意图。
- 精确 emoji 后光标一项使用 `setSelectionRange` 加真实 Shift 键，再由真实 React 事件读取并确认；**不宣称 Edge 单击手势验收**。旧的键盘／鼠标前置失败及最小 readonly textarea 对照诊断保留。
- 首轮统一编译发现新测试误用 DbClient 方法；第二轮两项新测试按旧固定日期查询使用量失败。改为正确查询后通过。原始编译／测试日志保留，不作为生产语音故障。

## 冻结音频与真实云端

使用此前固定 TTS WAV，16 kHz／PCM16／mono；仅正常用户环境读取已授权百炼配置，凭据不进入输出。北京 `qwen3-asr-flash-realtime` 与 `qwen-flash`，空词典、不自动重试。没有打开麦克风、读取用户历史或执行自动输入。

| 固定输入 | SHA256 | 样本／时长 |
| --- | --- | --- |
| B：预算改口、条件否定、周五试用尚未确定 | `6692ed05e4eceb2809535361de70bff15be7d076af2836812eab43733ac41700` | 302,160／18.885 s |
| C：两项行动、30 秒数字静音、末句否定 | `1abb6a60c678755d4e281bfb2e1a64a4ace32855ec62b17ea2e1b3c320f6785e` | 665,760／41.610 s |

初始预登记 4 ASR＋最多 2 rewrite：实际执行 2 次 burst 生产 retry 与 2 次按时长发送的独立 feeder；burst 两次均因实验未保存自造 Mode 而在 checkpoint 返回 `notFound`，因此零 rewrite。这个 fixture 缺陷已修复：读取隔离库合法内置 Mode 完整版本，真实 checkpoint 无云 preflight 在读取用户配置／凭据前执行。旧失败保留。

**burst 的识别缩短发生在上述 checkpoint 之前，仍是独立有效证据。** B 全部 302,160 样本、C 全部 665,760 样本均实际写出，Finish／session.finished 已收到、pending 为零，但各只有 1 final，分别为“文。”和“第一项。”。`complete=true` 表示协议 final 收齐，不能证明 WAV 内容完整。

独立实时 feeder：B 完整识别，C 在约 44.884 s 返回 `asr_service_error`，实验按预登记规则停止；C 的失败不补跑覆盖。这不是正常实时路径缺 final 根因已修复的证据。

随后单独预登记修复候选的 2 ASR＋2 rewrite。**本轮实际共 6 次 ASR、2 次 rewrite**，没有自动补跑。候选直接调用生产 `AppHub::retry_asr`，相同 B/C PCM、模型和词典；不同于初始候选的是配置 ACK 等待和实时回放节拍，以及合法的实验 Mode 前置配置。

| 候选真实 retry | socket 帧／真实样本／补零 | pending／final | 总耗时 | 整理往返 | 结果 |
| --- | --- | --- | --- | --- | --- |
| B／结构 | 189／302,160／240 | 0／10 | 21,359 ms | 933.683 ms | Ready；预算 3,500 而非 5,000、未确认前不邀请、可能周五试用但时间未定全部保留 |
| C／轻整理 | 417／665,760／1,440 | 0／6 | 45,304 ms | 604.717 ms | Ready；检查麦克风、保存到本地和停顿后的“不删除原始录音”全部保留 |

每个候选只调用一次整理、`creationReason=retry`、零 delivery_attempts，隔离 DB 与 worker 释放。`isolatedUsageCount` 是当日快照：fixture 的结束日期固定在 10 月 4 日，因此 10 月 8 日的数值为零；**不解释为没有写入一次 usage event**。一次计数契约由独立 Store／会话回归验证。

B 前导语“嗯”仍被识别为“文。”，成稿删除了该冗余；C 来源与最终稿保留三项内容。两份成稿由 Codex 按预登记规则复核，不是独立真人标注、一般准确率或 HCI 研究。仍为一个成稿块对应全部来源，没有实现精确逐 bullet 映射。

诊断争用丢弃数 B=1、C=3，最终累计写出和终止字段均在后续快照保留。诊断使用 try_lock 避免阻塞采音，但不是零开销；缺失字段且 `droppedEvents>0` 时不能推断服务端未发出事件。固定事件类型最多 14 类、tail 最多 32 条、trace 最多 64 条；诊断不含正文、音频或 Key。

实时节拍与[官方 Qwen PCM 文件输入示例](https://help.aliyun.com/zh/model-studio/real-time-speech-recognition-user-guide)一致。它使重试耗时接近原音频长度；尚未评估更快且可靠的倍速或非实时 ASR 路线。正常录音速度、真实人声和总体 P95 尚未测量。

## 真实输入的未完成部分

自有 Edit／RichEdit 8 场景均在 foregroundReadiness 前置失败，未进入产品 capture／dispatch，productPathFailures=0。8 个 fixture 均有退出记录。标准 `AllowSetForegroundWindow` 许可交接没有解决当前环境的前台限制；没有模拟 Alt、AttachThreadInput 或强拉用户焦点。

隔离 Edge 首次工具因旧 psutil API 不兼容退出，修复后两轮在启动 PID 提前退出时结束。独立 aftercheck 找到两份全新 UUID profile 的重父化自有 Edge，按 EXE、完整 profile 参数、PID／创建时间精确核对后清理，未触及用户浏览器。工具补 broker 生命周期核对后第四轮仍在 12 秒启动期限内未验证出 owner；该轮精确 profile aftercheck 为零。四轮实际均 **0/9 场景**，未进入产品输入路径。所有失败及清理收据保留，不把前置失败当成产品输入通过／失败。产品仍使用 Unconfirmed；`SendInput` 事件数量不等于宿主正文已插入。[Microsoft SendInput](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-sendinput)。

因此本轮没有新增真实跨应用输入通过证据，真实右 Alt、浮层、麦克风、管理员宿主、完整用户任务、HCI 与安装／升级／卸载仍待验收。

## 本机证据

本机目录：`D:/Codex_Project/SpeakMode_Dev/evidence/voice-0.1.7-2026-10-08-r1/`，不提交音频、运行数据或密钥到 Git。

- `rust-checks-r4/rust-checks.json`、`lib.log`、`examples-tests.log`、`examples-build.log`、输入前后摘要；r1/r2/r3 保留中间结果。
- `frontend/07-production-smoke/receipt.json` 与截图；早期失败、选择诊断、最小原生 textarea 对照保留。
- `cloud/registration.json`、`realtime-manifest.json`、`retry-manifest.json`、两个原始结果及单独合成文本；`paced-registration.json`、`retry-paced-manifest.json`、候选结果与单独文本。
- `native-edit-result.json`、`native-edit/`；四份 `native-browser-result*.json` 与隔离 fixture；`owned-browser-aftercheck.json`／`owned-browser-cleanup.json` 精确回收重父化自有进程。
- `bundle/receipt.json` 记录正式 release／NSIS，产物核对与交付另见版本计划。若构建尚未结束，不将目录存在当作成功。
