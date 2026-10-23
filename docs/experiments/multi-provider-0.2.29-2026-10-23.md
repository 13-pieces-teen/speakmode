# 0.2.29：独立识别与整理供应商

日期：2026-10-23。基线为 `main` / `ca7087d`（0.2.28）；桌面 0.2.29，前端 0.4.27。

## 用户行为

设置 → 云端服务分别选择语音识别与文字整理供应商。保存配置后，再保存各自的 Key；已保存凭据只显示状态，不回填明文。比如可以用阿里云 Qwen 识别，用 DeepSeek / Kimi / 智谱整理；也可以用 Google 识别，用阿里云整理。

| 阶段 | 供应商 | 本次接入 |
|---|---|---|
| 语音识别 | 阿里云百炼 | 保留 Qwen realtime、Audio 3.1 task streaming |
| 语音识别 | Google Gemini | `gemini-3.5-transcribe`，结束录音后一次提交完整 WAV，无实时字幕 |
| 文字整理 | 阿里云百炼 | 保留现有 Qwen 模型 |
| 文字整理 | DeepSeek | `deepseek-flash`、`deepseek-v4-pro` |
| 文字整理 | Kimi | `kimi-k2.6` |
| 文字整理 | 智谱 | `glm-4.7`、`glm-4.7-flashx` |
| 文字整理 | OpenAI | `gpt-5.4-mini` |
| 文字整理 | 自定义 OpenAI 兼容服务 | Base URL、模型 ID、独立 Key；服务需兼容 Chat Completions 和 JSON object 输出 |

阿里云两阶段共用所选地域、业务空间与同一个账户 Key。其他供应商分别保存，切换回来可继续使用对应系统凭据。更换供应商、地域、空间或自定义地址时清空未保存的密码草稿；配置冲突不覆盖其他更改。引导页使用同一配置组件，继续以两阶段均已配置作为练习就绪条件；引导完成与否不影响日常功能权限。

Google 使用专用转写接口的 `verbatim` 模式，保留原始识别内容，再交给独立整理服务。产品单次上限7分钟，为 inline 请求的 Base64 和元数据留出大小空间；这不是 Google 服务自身最大时长。取消、缺帧、错误尾序号、超时、拒绝或不完整响应均不能产生完整来源。

## 协议与凭据边界

供应商目录声明每阶段模型与 ASR adapter；文本的 OpenAI 兼容格式不代表语音接口也兼容。ASR 和整理各自验证 endpoint，因此历史重新整理不依赖无关识别模型、地域或识别 Key。预设供应商固定 origin；只有显式自定义服务可配置地址，要求 HTTPS（HTTP 仅限 loopback），拒绝地址内凭据、查询参数、片段、控制字符，并禁止 HTTP 重定向。

顶层旧 `provider` 等字段继续表示识别服务；新增 `rewriteProvider`、`rewriteEndpoint`。旧配置缺少这两项时默认阿里云，不改变原有 `aliyun:{region}:{workspace}` 凭据账户名称。新增字段保存后不保证旧二进制能重新读取配置；SQLite 表结构未改变。API Key 仍留在 Windows 凭据管理器，自定义服务按规范化完整 endpoint 的 SHA256 隔离账户。

开始录音时同时固定配置、词典和两份 Key；ASR 只能收到识别 Key，整理只能收到整理 Key。保存恢复、整理重试、历史候选及预览同样按阶段取 Key，明确的重试继续绑定原会话配置；不静默切换供应商。来源仍记录实际 ASR 供应商，诊断单独记录整理供应商，自定义地址和模型名称不进入诊断导出白名单。

所有生成、复核、局部修复和最终复核在统一 HTTP 出站边界适配参数。Aliyun 保持现有请求；OpenAI 使用 `max_completion_tokens` 和 `reasoning_effort:none`；DeepSeek/Kimi/智谱使用各自支持的非思考参数，Kimi 不发送不兼容的温度；custom 只发送通用参数。非 Aliyun 不发送 DashScope 工作区头。继续使用 JSON mode 与本地严格解码，未改为 native strict JSON schema。

Qwen 已评测的加速协议仍限定阿里云对应型号及地域；新供应商的场景写作使用原有来源、请求、修复与复核约束。旧 light/structure 路径保留本地来源、数字、逐字内容及有限意义检查，不能据此声称有同等语义复核或质量保证。14个旧云端实验显式限定原实验供应商范围，避免 raw HTTP 脚本绕过阶段凭据适配。

Google 批量 HTTP 没有可观察的 WebSocket 写入时点，其 socket write 计数保持未观测默认值、finish write 时点为空；不能以这些字段推断 Google 上传或服务处理耗时。采音出队数量仍可诊断，已剥离的 padding 不记作已上传。

## 验证

本地 HTTP / IPC / SQLite 与合成浏览器验证，不使用新供应商真实账号，不录真人音频，不向宿主输入：

- 完整 Rust 回归890项通过、0失败、4项显式忽略（需要额外条件）。覆盖 Google 有界 HTTP 转写、运行中取消、尾帧/缺帧、超大分块响应、重定向，跨供应商四阶段请求、凭据作用域、旧配置读取及会话快照。
- 30个 examples 测试目标共118项通过，0失败；只执行本地测试，不调用云端实验 main。
- Python 诊断导出38项通过，包括新供应商白名单与自定义地址/模型脱敏。
- TypeScript 和前端生产构建通过；22项供应商设置及引导页浏览器检查通过，无页面错误或外部请求。发现并修复更换供应商时 Radix 模型选择瞬时清空导致保存失效的问题；验证独立 Key、草稿清理、版本冲突及720px键盘交互。
- Windows MSVC release EXE / NSIS 安装包构建通过（退出码0），缓存11.08 GiB，磁盘预算无违规。首次打包编译器进程异常退出；复核 rustc/cargo 后重跑成功。未安装或替换正在使用的桌面应用。

未运行：新供应商真实账号连通性、真实语音准确率、中英混说保真、速度/费用对照、Windows 物理快捷键到宿主输入全链路。接入目录意味着已实现协议适配，不能作为这些指标的通过结论。旧阿里云速度报告的结论不外推到新供应商。

## 官方接口核对

本轮核对2026-10-23可访问的官方文档，模型权限及可用范围仍以账号实际口述结果为准：

- [Google 专用转写](https://ai.google.dev/gemini-api/docs/transcribe)、[Interactions API](https://ai.google.dev/api/interactions-api)
- [DeepSeek Chat API](https://api-docs.deepseek.com/api/create-chat-completion/)、[JSON mode](https://api-docs.deepseek.com/guides/json_mode/)
- [Kimi K2.6](https://platform.kimi.com/docs/guide/kimi-k2-6-quickstart)、[Kimi JSON mode](https://platform.kimi.com/docs/guide/use-json-mode-feature-of-kimi-api)
- [智谱 GLM 4.7](https://docs.bigmodel.cn/cn/guide/models/text/glm-4.7)、[思考模式](https://docs.bigmodel.cn/cn/guide/capabilities/thinking-mode)
- [OpenAI GPT 5.4 Mini](https://developers.openai.com/api/docs/models/gpt-5.4-mini)、[Chat Completions](https://developers.openai.com/api/reference/resources/chat/subresources/completions/methods/create)

本机证据保存到工作区外层 `output/validation/multi-provider-2026-10-23/`，不随源代码提交。
