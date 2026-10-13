# 0.2.20：统一整理期限、输入调查与文件恢复试点

日期：2026-10-13。实施基线为 `79b317f2ce5e723ab36c96c49c0cc83780df124f`（桌面 0.2.19／前端 0.4.18），交付候选为 **桌面 0.2.20／前端 0.4.19**。对应[阶段计划](../plans/typeless-rea-implementation-2026-10-13.md)。本轮完成 A，完成 B 的环境调查与受控输入子集，完成 C 的隔离工具与三条合成音频试点；正式真人/竞品时序和预留集未执行。

## 已交付的生产改动

- 协调器按冻结来源选择原有 5／8／20 秒预算，创建唯一绝对截止时间，传入 dispatcher、minimum、fluent 和旧单次请求路径。修补继续要求至少 1,400 ms 剩余时间，下层不重新计时。
- 显式重试、历史候选、Mode 试用保留原有 20 秒策略。生产模型、提示词、词典快照、ASR、Mode 规则和自动输入许可没有调整。
- 每次请求记录 generation / review / repair / finalReview 的开始、结束、耗时、发起时剩余预算和固定结果枚举；每轮最多四条。取消或外层超时丢弃 future 时也关闭计时。`responseReceived` 仅表示响应读取完成，整理是否通过仍看整个 rewrite 的独立结果。
- 诊断保持内存有界与非阻塞；不记录正文、提示、响应、Key 或实验 Trace。导出器保留旧收据兼容性，过滤任意字段并拒绝畸形时间及超长请求列表。
- `requests` 记录客户端请求尝试，不充当服务端收到请求或计费的回执；下表请求数另由本地 HTTP 服务实际收取并核对。
- 取消不生成可交付文本；正常初次整理超时仍只取消整理子 token，由完整识别稿保底；迟到同步解析、来源身份、单次交付、手动重试不自动输入等检查继续生效。

代码：[会话协调](../../app/src-tauri/src/engine/session.rs)、[执行期限与观测](../../app/src-tauri/src/cloud/rewrite_execution.rs)、[请求分发](../../app/src-tauri/src/cloud/rewrite.rs)、[诊断](../../app/src-tauri/src/engine/diagnostics.rs)、[导出器](../../scripts/export-voice-diagnostics.py)。

## A：可控 HTTP 对照结果

测试实际经过协调器 → 生产 dispatcher → pipeline → 本地 HTTP → SQLite。旧逻辑对照臂只在 cloud 入口注入原先的 20 秒时钟；修复臂使用协调器传入的 deadline。生成和首次复核故意耗至不足 1.4 秒，第三个响应设为迟到。

| 来源字符数／原预算 | 旧逻辑实际请求数 → 修复后 | 完成时间（旧 → 新） | 输出 |
| --- | --- | --- | --- |
| 80／5 秒 | 3 → 2 | 5.002 → 3.924 秒 | 两臂均完整原稿保底，各一次 |
| 401／8 秒 | 3 → 2 | 8.003 → 6.934 秒 | 两臂均完整原稿保底，各一次 |
| 801／20 秒 | 2 → 2 | 18.940 → 18.947 秒 | 两臂均完整原稿保底，各一次 |

收益：短、中场景各少一次赶不上期限的修补请求，约早 1.07 秒回到完整原稿。长文两层原本同为 20 秒，未观察到收益。这是可控故障场景的工程结果，不能作为正常口述整体提速、整理质量提升或真实费用下降百分比。

另测正在等待 HTTP 时的实际截止：收据为 `timedOut`、请求停止、只保存一次原稿。旧错位时钟被外层中断时记录 `interrupted`，不伪称内部自行超时。原有字符边界、取消、会话替换、迟到同步检查和重复交付测试保持通过。

复现入口：[真实调用路径测试](../../app/src-tauri/src/engine/rewrite_deadline_tests.rs)。[成对原始收据](D:/Codex_Project/SpeakMode_Dev/output/validation/typeless-rea-implementation-2026-10-13/deadline-pairs.json)。表中使用第二轮成功运行；最终 0.2.20 回归再次通过同一实验。

## B：输入行为与 REA 调查

REA 6.4.0 / Node 24.19.0 的 `doctor`、`providers`、指定 InputHelper 的 `inspect --provider ghidra` 已实际执行。原生 inspect 返回 `provider_unavailable`：未配置 Ghidra，当前选中的 Java 8 为 32 位运行时，未满足 Ghidra 12.1.x 与 64 位 JDK 21+ 条件。未安装工具链或修改全局 REA 注册。[REA 官方 Windows 边界](https://github.com/morluto/rea/blob/main/docs/windows-ghidra-p0.md)

只读 PE 检查锁定 InputHelper.dll SHA-256 为 `897585ae414e5a83a4a0aa649447291bb4711909b4460800674e91a418ed54d5`。

| 导出 | RVA | 已知范围 |
| --- | --- | --- |
| `getCurrentInputState` | `0xF440` | JS 调用边界和导出身份确认；内部策略未知 |
| `getSelectedTextBySimulateCopyAsync` | `0xF460` | 导出身份确认；焦点、选区与恢复细节未知 |
| `insertRichText` | `0xF470` | JS 包装和导出确认；原生返回值具体含义未知 |
| `insertText` | `0xF480` | JS 原生返回 1 后等待 500 ms；不能证明宿主插入确认 |

DLL 导入了 `SendInput`、`SetClipboardData`、`GetClipboardData`、`OleSetClipboard`、`OleFlushClipboard` 和 `CoCreateInstance`。这说明相关能力存在，**不能据导入表认定某一导出采用了哪条路径、具备 UIA 回读或为何等待 500 ms**。未把竞品二进制或解码源码复制进产品仓库。

SpeakMode 当前路径使用自有测试控件回读验收：Windows Edit/RichEdit 与只读/密码/光标或字段变化等 **8/8**；隔离 Edge input、textarea、contenteditable 与只读、密码、非编辑目标等 **9/9**。正向插入在测试宿主回读确认；生产回执仍是 `Unconfirmed`。重复输入被拒绝，浏览器自有进程清理完成。

这些 17 个现有场景不等同计划的 30 格宿主矩阵：普通中英文覆盖了前三组的一部分，emoji、长多行、选区替换、真实换窗/取消及 Word/飞书/VS Code 专项尚未执行。[矩阵](D:/Codex_Project/SpeakMode_Dev/output/validation/typeless-rea-implementation-2026-10-13/host-matrix.csv)逐格标为 PARTIAL／NOT_RUN。当前没有证据支持新增输入策略或在未确认派发后自动补发。

B2 的 12 段核对录音、两产品、正常/慢网最多 48 次完整会话未执行：现有材料是 TTS，且没有本轮 Typeless 运行时观测。下节的 ASR 工具测量不计为麦克风、快捷键、UI 或跨应用完整时序。

## C：隔离文件恢复工具及接入决定

新增 [file_asr_lab](../../app/src-tauri/examples/file_asr_lab.rs)，仅显式读取指定 PCM16/16kHz/mono WAV、只读配置和现有应用凭据，调用北京 `qwen3-asr-flash` 的同步文件接口。候选不加入默认模型菜单，不改生产 Retry，不写历史、不操作输入。官方支持 Base64 Data URL，本工具还检查编码后 10 MB 和 5 分钟上限；16kHz PCM 的编码大小可能先达到限制。[官方接口说明](https://help.aliyun.com/zh/model-studio/non-realtime-speech-recognition-user-guide)

只有模型身份正确、`finish_reason=stop`、非空完整结果才能生成新的实验来源，实际记录 `qwen3-asr-flash` / `qwen-file-lab-v1` 与新 attempt。文件响应无时间戳时保持空值。客户端无重试、无重定向，60 秒绝对期限；取消、空音频、畸形/过大响应、HTTP 错误和超限均有隔离测试。输出先独占创建，再访问凭据/网络；失败和开始记录都留在本机 JSONL。

复用三段既有 TTS（词典关闭，auto 语言），对照当前生产 `qwen3-asr-flash-realtime` 的完整实时速度重放，不删除 pacing sleep。累计 **6 次真实 ASR 请求：3 文件＋3 实时，0 整理请求，0 自动输入**。没有云端重试。素材及 SHA 在调用前登记。

| 素材 | 时长 | 实时重放完成 | 文件识别完成 | 脚本 CER：实时 → 文件 |
| --- | --- | --- | --- | --- |
| A：技术术语 | 9.40 秒 | 10.609 秒 | 0.795 秒 | 16.07% → 19.64% |
| B：预算改口、否定和不确定性 | 18.885 秒 | 20.815 秒 | 0.863 秒 | 1.69% → 1.69% |
| C：长停顿后尾句 | 41.61 秒 | 45.613 秒 | 0.947 秒 | 0% → 0% |

三例耗时中位数 **20.815 → 0.863 秒（降低 95.85%）**，这是已有完整录音的恢复成本，不是正常实时听写停止后的等待。实时臂 Finish 入队到最终完成分别 43／43／53 ms，也不是服务端 ACK 或用户可见时延。

计时范围为工具内识别调用：文件臂在 WAV 校验、编码、配置预检后开始，包含凭据读取与 HTTP；实时臂从 ASR 连接/发送任务开始，包含按实时速度发送完整音频。两臂均不含原始录音、前置文件读取、UI 与历史写入；不能把此工具计时直接当作未来 Retry 按钮到页面显示的耗时。

CER 口径：NFKC、忽略大小写/空白/标点，以既有 TTS 脚本为参考，逐字符 Levenshtein；不是人工听写标注集。150 个归一化参考字符共 10 → 12 处编辑距离，汇总 **6.67% → 8.00%**。A 中两臂都错认 WASAPI、Rubato，文件臂编辑距离更大；B 的最终预算、旧预算否定、暂不发邀请和不确定时间保留；C 尾句“不要删除原始录音”两臂保留。

**决定：保留实验工具，继续研究，不接入正式重试。** 文件路径有明显速度潜力，但本次术语样例退步，且缺 12 条独立预留、60/180 秒档和等价词典测量，未达到原计划的质量及证据门槛。C5 维持 CONDITIONAL。52 次总请求上限本轮已用 6 次，不自动追加完整 48 次正式比较。

保留的实验偏差：实时工具的输出目录约束造成 4 次本地预检失败，均发生在凭据和云请求之前；修正输出位置后才运行实时臂。原定交错顺序变为三个文件臂后接三个实时臂，已留修订收据，不能当作严格交错实验或独立预留成绩。初次 Rust 对照测试还纠正了旧时钟 `interrupted` 状态的断言；原失败日志保留。

[试点结果与原文](D:/Codex_Project/SpeakMode_Dev/output/validation/typeless-rea-implementation-2026-10-13/recovery-pilot-evaluation.json) · [预登记](D:/Codex_Project/SpeakMode_Dev/output/validation/typeless-rea-implementation-2026-10-13/recovery-pilot-preregistration.json) · [实时原始收据](D:/Codex_Project/SpeakMode_Dev/evidence/typeless-rea-recovery-2026-10-13)。实验二进制为版本号更新前的 0.2.19 加本次期限修复，SHA 独立记录；随后只补验证、更新版本并构建 0.2.20 候选。

## 验证和交付

- Windows desktop 库最终 **743 passed / 0 failed / 3 ignored**；ignored 沿用已有测试。
- 文件实验工具 **5 项测试通过**；诊断导出器 **37 项通过**；TypeScript 检查通过。
- 受控 Windows 原生输入 **8/8**、隔离 Edge **9/9**；这不代表全部日常宿主验收。
- 使用既有单一 MSVC 缓存生成独立候选。EXE、NSIS、资源、源码输入稳定性和 SHA 以交付目录 `build-receipt.json` 为最终依据。候选构建时尚未提交或推送；后续 PR 独立记录。未自动安装、启动或发布候选应用。

[本机完整验证目录](D:/Codex_Project/SpeakMode_Dev/output/validation/typeless-rea-implementation-2026-10-13) · [候选交付目录](D:/Codex_Project/SpeakMode_Dev/output/releases/2026-10-13-deadline-recovery-0.2.20)。旧版本和用户资料保留。
