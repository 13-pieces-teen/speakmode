# SpeakMode

SpeakMode 是一个 Windows 语音输入 MVP：按右 Alt 开始口述，再按一次结束，将识别结果按所选 Mode 整理后输入到结束口述时的文本位置。录音中可以停顿、翻阅资料或移动光标。

当前桌面源码版本为 **0.2.21**，前端 package 版本为 **0.4.20**。仓库包含实际 React/Tauri/Rust 实现、锁定依赖、测试和构建所需的第三方材料；不包含个人配置、API Key、历史数据库、录音、安装包或本机工具链。

英文主导口述实验（未发布）：新增英文数值/单位与有限跨片段年份检查；英文成稿候选仍未解决真实样例回退，且出现错误年份通过模型复核，故候选只在默认关闭的 `experimental-english-dictation` 编译特性中保留。普通桌面构建继续使用v3提示词。见[实验与失败回归](docs/experiments/bilingual-dictation-2026-10-13.md)。

0.2.21 将应用表达偏好接入实际听写：Auto 可按结束口述时的应用选择本地呈现，聊天短列表转为自然句子，显式 Mode、个人规则和口述中的列表要求优先。新增设置开关、应用偏好、场景示例和浮层状态；只读取目标进程的程序名，不增加云端请求。775 项 Rust 测试和37项浏览器检查通过；本轮原生探针未取得前台权限，实际宿主输入、真人听写和安装升级仍待验收。详见[实施与验证](docs/experiments/app-aware-dictation-0.2.21-2026-10-13.md)。

0.2.20 将协调器的 5／8／20 秒截止时间传到全部整理阶段，避免时间不足时仍发修补请求，并增加不含正文的请求阶段计时。743 项桌面测试通过；受控慢请求中短文和中等长度各少一次请求，约提前 1.07 秒保底。文件恢复仅提供隔离实验工具：三段合成音频明显更快，但术语样例退步，尚未接入正式重试。详见[实施与效果评估](docs/experiments/deadline-recovery-0.2.20-2026-10-13.md)。

0.2.19 在百炼北京 / qwen3.8-flash 下统一各 Mode 的基础微整理，保留事实与主观语气，布局再按模式选择。增加一次受限局部修复、很窄的卡顿提示及精确纯删词本地验收；共同等待期限和单次原稿输入保底保留。历史候选继续行内对照、编辑与明确保存。真实文本实验仍有超时及保真回退，不宣称已达到 Typeless。见[实施计划](docs/plans/minimum-edit-0.2.19.md)与[验证记录](docs/experiments/minimum-edit-0.2.19-2026-10-13.md)。

0.2.17 接入较长中文的内容保护与全文整理，增加本地引语/代码检查、词典大小写和分开读出的缩写规范、词库发送元数据，并为中等长度口述使用 8 秒共享预算。31 次有限服务实验，用户长段在 13.15 秒完成整理；原始术语错误仍有边界，真人录音/粘贴尚需测试。固定音频对比后保留现有默认 ASR。见[本版报告](docs/experiments/dictation-upgrade-0.2.17-2026-10-12.md)与[实施计划](docs/plans/dictation-upgrade-0.2.17.md)。

0.2.16 将已验证配置的默认自动改为“全段自然整理 → 简短语义核对 → 一次交付”：北京 / qwen3.8-flash / 无个人规则时生效。43 次有限云端文本实验，独立预留核对 14/14，完整流程 5/6 在 5 秒内得到可用成稿，包括用户最近三段短例。长简历误拒及分段仍未解决，复核也不是保真证明；不代表真人听写准确率。详见[实验报告](docs/experiments/semantic-cleanup-0.2.16-2026-10-11.md)与[方案](docs/plans/semantic-cleanup-0.2.16.md)。

0.2.15 修复相邻重复数量与局部类型描述的误回退：已清理的两份用户样例响应由拒绝变为接受，并通过单片来源复验；此前 70 份响应判定保持不变。真实数量、所属对象和候选新增分类仍需来源见证，未改默认提示、模型或等待预算。有限检查仍有已知边界，不能当作完整保真证明；详见[本版报告](docs/experiments/game-dictation-0.2.15-2026-10-11.md)与[实施方案](docs/plans/game-dictation-0.2.15.md)。

0.2.14 针对本轮短口述的“模型已清理却被数量校验退回原稿”问题，新增限定描述构式的数量豁免，同时核对候选保留的对象、动作与来源。生产提示、模型、预算及一次性输入流程保持原实现。原始两次云请求、修复前后同响应重放、反例与分发状态见[本版报告](docs/experiments/filler-delivery-0.2.14-2026-10-11.md)，[方案文件](docs/plans/filler-delivery-0.2.14.md)记录范围与效果边界。

0.2.13 修复有限口语构式中的泛指数量、废弃起句、能力疑问及日期改口误拒，并保留不含正文的整理回退原因与耗时。68 次真实云端文字整理实验中，新提示的开发增益未在预留集确认，仍沿用生产提示、模型与等待预算；用户提供的长样例仍存在不确定性作用域的回退边界。详细增量、失败及分发状态见[本版报告](docs/experiments/natural-cleanup-0.2.13-2026-10-11.md)，[方案文件](docs/plans/natural-cleanup-0.2.13.md)保留预注册与调整记录。

0.2.12 在词典页增加“导入 Typeless”，读取本机已缓存的完整词表，自动去重并原子追加；如刚修改过 Typeless 词典，请先在 Typeless 打开“所有”页更新缓存。正常口述完成后不再显示“输入已派发／查看成稿”恢复卡片，实际失败仍保留必要操作。实现范围和来源限制见[本版计划](docs/plans/dictionary-import-quiet-delivery-0.2.12.md)，验证及交付状态见[本版记录](docs/experiments/dictionary-import-quiet-delivery-0.2.12-2026-10-11.md)。

0.2.11 按用户要求将默认整理模型更换为 Qwen 3.8 Flash，在云端服务设置中优先展示型号选择、已保存型号及待保存状态。现有北京地域旧默认 Flash 配置在新版首次启动时升级一次；用户随后回选 Flash 或其他型号会持续保留。语音识别、Key 与个人数据保持原配置，继续使用 `mode-contract-v1`；没有启用上一版的实验提示或宣称质量结论改变。详见[模型选择与升级方案](docs/plans/model-selection-0.2.11.md)。

0.2.10 修复“区别/差别”的否定误判、有限停顿导致的数值作用域误拦、严格相邻的数字改口撤回，并保护“下午两点前”等明确整点。保留一次性交付与完整识别稿回退。另完成 136 次真实云端合成文本实验，比较 Flash / Plus / 3.8 Flash 与整段自动整理协议；开发集的提升未在预留集的最终完整可用指标上得到确认，因此保留原默认模型和 Mode 契约，不将解析通过率冒充质量提升。见[方案及边界](docs/plans/automatic-dictation-0.2.10.md)、[完整实验与发行记录](docs/experiments/automatic-dictation-0.2.10-2026-10-10.md)。

0.2.9 实施[优先交付方案](docs/plans/delivery-first-0.2.9.md)：正常初次听写在完整识别稿保存后，整理异常自动使用识别稿继续一次性输入；短口述整理预算为 5 秒，长口述保留 20 秒。目标不可输入时可在浮层直接复制，无须打开主面板。回退来源与原因持久化，历史标为“识别稿”；修复 5 类已复现数值误拦，64 个正反例通过。真实自有 Windows 控件 8/8、隔离 Edge 9/9 输入检查通过；云端微整理候选仍需同时满足清理和保真，未因文字更短直接替换默认。完整证据和选型见[本版验证](docs/experiments/delivery-first-0.2.9-2026-10-10.md)，[初始调查](docs/plans/delivery-first-cleanup-2026-10-10.md)保留当时记录。

0.2.8 增加全局「轻语增强」开关，面向正常音调的轻声输入，默认关闭并持久化。使用受限增益和峰值保护，保留样本、长停顿、尾音和重试；诊断增加处理前后电平等标量，并支持同音频三臂与纯静音评测。工程验证通过不代表真人识别率提高；见[实施范围](docs/plans/soft-voice-0.2.8.md)与[实际验证](docs/experiments/soft-voice-0.2.8-2026-10-10.md)。

0.2.7 修复录音浮层 Mode 菜单的背景透视，改为 216px 宽的紧凑单行选择。个人 Mode 内部滚动；窗口尺寸与菜单定位就绪后显示，鼠标展开 150ms 小位移，键盘／减少动态效果即时显示，收起不延迟焦点返回。极限提示组合保留可见选项，实时字幕出现后保留正在浏览的 Mode。17 项菜单浏览器检查通过；使用合成 IPC，不等于真实 Windows 合成器、多屏 DPI 或语音验收。见[本版计划](docs/plans/recorder-mode-0.2.7.md)和[验证记录](docs/experiments/recorder-mode-0.2.7-2026-10-09.md)。

0.2.6 将词典收简为词卡网格、搜索与新词弹窗，批量添加移入弹窗，容量说明按需展开。保留原有保存、去重、版本冲突和草稿保护；修复弹窗遮罩、焦点返回及保存后切换编辑模式的版本衔接。26 项浏览器场景与 5 项容量组件检查通过，使用合成数据，未访问个人词库。见[本版计划](docs/plans/dictionary-ui-0.2.6.md)和[验证记录](docs/experiments/dictionary-ui-0.2.6-2026-10-09.md)。

0.2.5 修复有限跨 ASR 片段的否定、改口目标和疑问句检查，并增加成稿／停止到原生输入回执的耗时。原生 509 项通过，240 份既有响应离线重放；默认方案新增 5 份不再误拦，但完整任务仅增加 1 份。指定对象数量改口和中文逐位房号仍有已知限制。默认模型／提示不变，EXE／NSIS 已构建；真人听写和实际宿主输入未在本轮验收。见[本版计划](docs/plans/voice-0.2.5.md)与[实际结果和限制](docs/experiments/voice-0.2.5-2026-10-09.md)。

0.2.4 增加同输入轻整理／结构比较，历史试用保留真实识别分片；新候选可以对照生成前文本，并明确长文本整体高亮。修复一处合法主题归并被待确认限定检查误拦的情况。见[实施计划](docs/plans/mode-0.2.4.md)与[实际验证](docs/experiments/mode-0.2.4-2026-10-09.md)。两轮合计 48 次合成文本云端诊断未证明新提示能稳定提升整理质量，所以默认提示与模型保持原版，候选规则仅供显式实验工具调用。

后续完成[四方案筛选与独立确认实验](docs/experiments/mode-selection-2026-10-09.md)：20 条新合成输入、240 次真实请求。编辑单元＋本地排版 / Flash 在确认集任务通过 34/72，原版为 22/72；但候选有 3 次已放行的否定作用域错误，选择其作为后续研究路线，生产默认不变。测试工具和冻结语料已入库，实验结果不代表日常听写准确率。

试用入口：**Mode → 新建 Mode／编辑 → 试用效果 → 对比轻整理与结构**，无须先保存或命名。该按钮调用两次云端整理；正常自动口述在已验证配置的成功路径也调用两次（整理＋核对），其他方式与个人规则沿原单次路径。历史中 **换种整理 → 生成候选** 单次生成待核对草稿，默认展示改动；可逐处选择、直接编辑后保存，或取消候选。恢复与撤销、统一字体、今日／累计统计及原有资料结构保持，无数据库迁移。

## 功能

- 全局录音快捷键、Windows 原生悬浮栏、真实音量反馈和流式字幕。
- Auto、轻整理、结构与个人 Mode；个人规则控制格式，不能覆盖来源和输出契约。
- 按应用调整表达：停止口述时匹配本地程序名，支持自定义应用偏好；显式 Mode 优先，浏览器默认通用。
- 云端语音识别与文字整理，目前支持阿里云百炼北京/新加坡地域。
- 个人词典支持手工维护、批量添加、导入本机 Typeless 缓存词表和成稿中“纠词并记住”；词条保存后供下一次录音使用。
- 可选Audio3.1热词接口；结构整理支持块内主题归类，个人Mode可点选表达偏好。
- 历史记录、今日／累计使用量、识别稿对照、局部恢复及撤销。
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

开发热更新也应复用同一个 `CARGO_TARGET_DIR`，不要按版本复制编译缓存。桌面检查、测试和打包统一使用下方带磁盘预检的入口；目录选择、12 GiB 缓存预算及 8 GiB 可用空间要求见 [构建与缓存管理](docs/build-cache.md)。

在应用“设置 → 云端服务”中配置自己的 Key 和所属地域。新配置默认模型为 `qwen3-asr-flash-realtime` 和 `qwen3.8-flash`，可从模型列表另选型号并保存；模型可用性、额度和地域取决于你的账号。Key 通过 Windows Credential Manager 保存，不写入 Git 文件或发送到前端日志。

浏览器单独运行 `npm run dev` 可检查界面，但不生成示例数据或执行原生操作，不能代替桌面语音、系统快捷键或跨应用输入验收。

## 验证和打包

从仓库根目录运行前端合成 helper 检查：

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

合成 helper 不打开麦克风或请求云模型；Rust 测试使用项目专有 `.test-data/`。不要直接运行被忽略的 maintenance child 测试，它由父测试在受控目录启动。

统一脚本复用一份 MSVC target，禁用增量缓存，并在构建前后检查体积和磁盘余量；不会自动删除文件。打包直接调用本地 Tauri CLI，避免 PowerShell 的 npm 包装器吞掉 `--` 分隔参数。

验证状态与实际验收边界见 [docs/validation.md](docs/validation.md)。安装包构建、真实口述、实际宿主输入及安装/升级/卸载是不同的验证范围，不把源码测试等同于整体产品验收。

0.2.0实施听写质量升级：受支持的热词协议、纠词记忆、结构归类、有限新增数值检查、个人表达快捷偏好和同音频模型对照。固定TTS真实云端8/8识别完成，但新模型专名表现没有全面胜过旧链路，默认模型保留。初轮整理实验发现示例污染和问句变陈述，已移除事实示例并加强编辑规则；同素材Plus诊断4/4通过，Flash复杂改口仍有限制。详见[版本计划](docs/plans/voice-0.2.0.md)和[完整实际结果](docs/experiments/voice-0.2.0-2026-10-08.md)。

0.1.9历史交付优先修复公共自动输入链路：独立 Alt／Win 短按的菜单仲裁、绑定已记录目标的交付前检查，以及停止时输入位置失败的具体原因。414 项桌面库回归通过，程序与 NSIS 构建完成；真实物理右 Alt、Codex／记事本／浏览器输入仍待用户复测。详见 [版本计划](docs/plans/voice-0.1.9.md)和[实际记录](docs/experiments/voice-0.1.9-2026-10-08.md)。模型与整理规则没有调整。

0.1.5 的词典规则、采音改动、测量口径与实际口述对比见 [语音增量计划](docs/plans/voice-0.1.5.md)。

固定 TTS 的真实云端对比、默认麦克风采音与自有 Windows 控件实验见 [0.1.5 真实语音实验报告](docs/experiments/voice-real-lab-2026-10-08.md)。该实验发现的来源映射失败和时间限定遗漏已按 [0.1.6 修复计划](docs/plans/voice-0.1.6.md)实施；[新一轮复测报告](docs/experiments/voice-repair-0.1.6-2026-10-08.md)记录 22/24 任务产出、所有实际整理调用通过，以及 ASR 不完整和本轮宿主输入未验收的限制。`app/src-tauri/examples/` 中的实验工具须显式运行，正常程序启动不执行实验。

恢复、语音、输入与设置优化见 [优化清单](docs/plans/optimization-backlog-2026-10-08.md)。O01/O03 及 O02 的诊断和缓存重试节拍见 [0.1.7 计划](docs/plans/voice-0.1.7.md)和[实际报告](docs/experiments/voice-0.1.7-2026-10-08.md)。0.1.8 新增 O04、有限 O06 事实检查及模型配置兼容校验；其余条目继续按证据推进。

后续语音模型、词典、整理保真与真人质量评测见 [质量优化路线](docs/plans/voice-quality-roadmap-2026-10-08.md)。第一轮实施见 [0.1.8 增量计划](docs/plans/voice-0.1.8.md)与[评测工具](docs/experiments/voice-quality-lab.md)。默认模型保持原值；旧实时协议的快照及文本候选已列入兼容清单，0.2.0已接入Audio3.1 Task Streaming，其他新协议／文件模型仍禁用；真人与Typeless对照仍待执行。

针对用户提供的 SpeakMode／Typeless 文本差异，新增[自动听写对照计划](docs/plans/automatic-dictation-benchmark-2026-10-10.md)，含 8 条开发脚本、4 条预留验证脚本与 24 行空白结果登记表。重点评价最终交付文本的口语清理、专名大小写、语义保留和人工修改成本；材料尚未录音或运行，不代表竞品实测，也未修改生产默认。

手动诊断导出工具 `scripts/export-voice-diagnostics.py` 支持 schema1/2；只导出有界类型化元数据，过滤正文、PCM、Key 和原始服务端标识。使用 `--help` 查看要求。`--project-root` 必须显式指向包含 `app/src-tauri/tauri.conf.json` 的本机项目目录；RPC 仅允许经过 PID/可执行文件/CDP 归属核对的主窗口。导出失败不输出原始 payload。

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
