# 离线语音质量评测工具

维护日期：2026-10-08。工具入口：`scripts/voice-quality-lab.py`；Python 标准库，无额外依赖。它只读取显式指定的本地清单、WAV、标注和结果回执，不录音、不生成 TTS、不访问云模型、Key、个人数据库或目标输入框。本轮交付的是可重复评分流程，没有真人录音成绩、模型胜出结论或自动切换默认模型。

## 本轮完成范围

1. 严格检查 manifest/reference/receipt/review 的字段与 JSON 重复键；冻结源代码提交、arm 配置、音频文件 SHA256、PCM SHA256、16 kHz／PCM16／单声道规格及逐字参考文件 SHA256。
2. 在每次登记和评分前重读验证冻结输入，参考、规则、音频或清单变化会拒绝评测。修改标注必须新建实验；不能覆盖旧实验中的失败或提高旧成绩。
3. 按 fixture × arm 预登记一次 primary；未采集、未调用、调用失败、未完成人工评审都保留分母。诊断复跑使用新 runId，并关联同条 primary，单独统计，费用纳入全调用成本。
4. 人声和 synthetic、dev 和 blind test 分组。同一来源种类下，同一个匿名 speakerId 不得同时用于 dev/test。必须由采集者确认 speakerId 对应同一个真实人，工具不能识别人声或验证录音身份。
5. 中文 CER、英文 WER、混合文本英文投影 WER、术语正／负例、双人事实与 Mode 评审、可用成稿比例、延迟 P50/P95、已知／缺失成本。统计默认不导出逐字稿、成稿、事实描述或本地文件路径。

未实现：云端执行器、新 ASR 协议、模型打分器、配对置信区间、标注 UI、声道处理、运行时来源恢复。不能把本地边界测试解释为真人质量或真实麦克风验收。

## 先生成 24 条采集任务

以下 PowerShell 命令在仓库根目录运行，`$labRoot` 应指定独立本机实验目录。音频和含正文标注应保留在本机；仓库仅包含规则和虚构标注示例。

```powershell
$labRoot = 'D:/SpeakModeVoiceQuality/pilot-24'
$sourceCommit = git rev-parse HEAD
python scripts/voice-quality-lab.py init --directory $labRoot --source-commit $sourceCommit
```

生成 `manifest.json` 和 `collection-tasks.json`：八类各三条，所有音频和参考路径为 null、collectionStatus 为 missing；没有成绩。模板安排 8 条 dev／16 条 test 和彼此分离的匿名说话人占位符，采集时替换成真实受试者编号。至少 6 名说话人／120 条正式集需要另行预登记，模板不是正式盲测集。

采集任务中的提示只帮助覆盖反例，**不能当参考逐字稿**。标注实际说出的内容及实际改口，长停顿保留真实 30–60 秒静音。不要生成 TTS 来填真人 missing 项。操作者必须先填写实际模型、地域、适配器、参数、词典 hash 和提示词 hash，删除 `parameters.templateOnly`；这个标记会阻止登记非 missing 调用。`sha256(b'')` 仅用于确实为空的词典等输入，不能当未收集配置的占位证明。

## 输入契约和冻结

manifest 顶层固定为 `schemaVersion, experimentId, sourceCommit, arms, fixtures`。sourceCommit 为完整小写 40 位 SHA。每个 arm 有 `id, task, mode, config`：

| 字段 | 固定语义 |
| --- | --- |
| task | `asr`、`rewrite`、`pipeline`；分别评识别、独立整理、完整模型链路 |
| mode | asr 必须 `none`；其他必须 `light`／`structure`，分别预登记 |
| config | `asrModel, rewriteModel, adapter, region, language, dictionarySha256, promptSha256, parameters`，均需显式提供；不适用模型写 null |
| parameters | 明确记录实际 VAD、帧长、节拍、温度、输出契约等；工具冻结 JSON，不推断协议或参数有效性 |

fixture 固定为 `id, speakerId, split, sourceKind, category, tags, collectionStatus, audioPath, referencePath`。ready 条目必须同时提供两个绝对路径；missing 必须两个路径均 null。预登记后不应删条目、改变 speaker/split、补录音后沿用旧冻结文件。

reference 的字段见 [虚构格式示例](../fixtures/voice-quality-reference.example.json)。必须填写逐字稿、语言、可删口头语、所有有效与撤回事实、关键事实及来源位置、正负词条、light 和 structure 的评分条件。来源范围使用 Python Unicode 字符下标 `[sourceStart, sourceEnd)`，相对于未经正规化的原逐字稿。`withdrawn` 事实判 correct 的意思是正确处理已撤回信息，例如最终预算3500，而非把5000恢复成有效预算；保留有意义的对比是否必要应写进 description。每条至少一个保留的关键事实，每个 Mode 至少一个评分条件。

同一录音文件可以被所有 arm 引用；16 kHz／PCM16／mono WAV 最大十分钟，冻结同时检查实际 PCM 字节与 WAV 声明帧数一致。保存其他格式的原始录音可另留一份，但参评格式与重采样方法须固定，不裁停顿。参考或音频改动后，新实验会得到新 hash，已有结果不会混入。

```powershell
python scripts/voice-quality-lab.py freeze --manifest "$labRoot/manifest.json" --output "$labRoot/frozen.json"
python scripts/voice-quality-lab.py verify --lock "$labRoot/frozen.json"
python scripts/voice-quality-lab.py registrations --lock "$labRoot/frozen.json"
```

registrations 输出匿名 fixtureId／armId、registrationId 和 configSha256，不输出文本。每个 registrationId 是 fixture/arm 二元组 hash 的前24位；它用来登记同一输入的结果。ready 实验应在首次调用前冻结。仅 missing 的模板也能冻结以验证流程，但采集完成后必须新建完整实验冻结文件。

## 同音频结果登记入口

执行器由操作者选择，工具本身不会发请求。调用后，将结果整理成如下本机 `draft.json`。下面是字段说明用例，不是已执行成功的证据；registrationId 和 executedConfigSha256 必须换成真实冻结输入对应的值。`executedConfigSha256` 复制 registrations 输出的 configSha256，并确认实际调用配置一致；它使用工具 `digest(arm.config)` 的排序键／紧凑 UTF-8 JSON 定义。无法获得服务端实际版本时留 null，不能把请求别名猜成快照版本。

```json
{
  "schemaVersion": 1,
  "runId": "pilot-01-baseline-first",
  "registrationId": "REPLACE_WITH_REGISTRATION_ID",
  "purpose": "primary",
  "diagnosticOf": null,
  "status": "succeeded",
  "asrText": "实际识别稿，仅保存在本机",
  "outputText": "实际整理成稿，仅保存在本机",
  "protocolComplete": true,
  "latencyMs": {"speechToCaption": null, "stopToAsr": null, "stopToReady": null, "stopToDispatch": null},
  "cost": {"currency": null, "amount": null},
  "errorCode": null,
  "executedConfigSha256": "REPLACE_WITH_ACTUALLY_EXECUTED_CONFIG_HASH",
  "actualAsrVersion": null,
  "actualRewriteVersion": null
}
```

`status` 只接受 `missing, failed, succeeded`。missing 不能填写文本、测量、版本或费用；失败可以保存实际 partial，但永远不算可用成稿。工具加上冻结输入 bindings、recordedAt 和 resultSha256，以排除不同输入或结果改稿。返回值仅包含 runId/registrationId/purpose/status。

```powershell
python scripts/voice-quality-lab.py record --lock "$labRoot/frozen.json" --input "$labRoot/draft.json" --receipts "$labRoot/receipts"
```

同一个 registration 不能登记第二次 primary；复跑设 `purpose: diagnostic`、新 runId、`diagnosticOf: 原primary的runId`。即便复跑成功，原失败仍留在主结果。保存文件采用独占创建，默认不覆盖文件；重写 receipt 会被 resultSha256 检查拒绝。hash 是本地一致性检查，不是不可伪造的运行证明；工具不能证明模型确实收到指定音频、审稿者身份独立或回执来自真实服务。

现有 [voice_lab](../../app/src-tauri/examples/voice_lab.rs) 与失败回放测试可继续提供同 PCM 实验回执。历史 A/B/C 必须登记 `sourceKind: synthetic`；reference 来自冻结历史原文，不是 cloud 输出。人工选取 ASR `source.items[].text` 和对应 rewrite 输出填入新 receipt，不导入 provider 整体响应。旧工具的 `finishEnqueuedToAsrEndMs` 是实验队列到 ASR 终点代理，不能直接填 `stopToAsr`；完整成稿、派发和成本没有测量时均留 null。本轮未重新运行旧工具、测试语音或云端。

独立整理 A/B 需由执行者提供同一冻结人工逐字稿或同一固定实际 ASR 稿，并在 parameters 记录该输入 SHA；工具会比较配置 hash，但没有独立的逐条 rewrite 输入执行证明。完整 pipeline 的 ASR/整理错误不能通过单一 CER 归因，建议识别阶段另立 task=asr 的 arms。

## 双人事实与 Mode 评分

仅 succeeded 条目允许人工评分。两名独立人类先看同一原音频／来源／候选文本，隐藏模型和 arm 身份；操作上随机化顺序，blind=true 是操作者声明，工具没有盲化 UI。每条事实与每项指定 Mode 条件必须填 `correct, incorrect, uncertain`，新增未经来源支持的事实用 addedFacts=true。需要双人完全一致且没有 uncertain 才出确认结果；分歧或 uncertain 交第三名独立 adjudicator，保留原评分。

```json
{
  "schemaVersion": 1,
  "runId": "pilot-01-baseline-first",
  "reviewerId": "human-rater-01",
  "role": "reviewer",
  "blind": true,
  "factVerdicts": {"conditional-action": "uncertain", "uncertain-date": "uncertain", "product-name": "uncertain"},
  "modeVerdicts": {"clean-filler": "uncertain", "natural-paragraph": "uncertain"},
  "addedFacts": false
}
```

以上评分对应虚构 reference 示例的 light rubric；真实文件的 keys 必须逐项一致。事实不能只评分关键词命中，需核对条件、对象、否定、数值单位、最终改口和尾句。模型自评不能冒充人类 rating。每次 `review` 保存独立 reviewerId；复用同一人或重复评分被拒绝。adjudicator 使用第三个 reviewerId，role 设 adjudicator，必须已有两份原评分。

```powershell
python scripts/voice-quality-lab.py review --lock "$labRoot/frozen.json" --receipts "$labRoot/receipts" --input "$labRoot/rating.json" --reviews "$labRoot/reviews"
python scripts/voice-quality-lab.py evaluate --lock "$labRoot/frozen.json" --receipts "$labRoot/receipts" --reviews "$labRoot/reviews" --output "$labRoot/scores-001.json"
```

评价输出文件也独占创建。再次统计另选 scores-002.json。原始标注／候选全文只存在明确输入文件；汇总 JSON 有匿名条目编号、计数和指标，不导出全文或路径。

## 指标规范 v1

| 指标 | 算法与分母 |
| --- | --- |
| 中文 CER／mixed CER | NFKC+casefold，去 Unicode 标点和空白，保留其余全部字符包括数字和拉丁字母；字符 Levenshtein 总 edits ÷ 总参考字符。不做数字、拼写或同音纠正。混合 CER 是全字符分数，不是纯中文字分数。 |
| 英文 WER | NFKC+casefold，Unicode `\w+` 分词；标点分隔。词 Levenshtein 总 edits ÷ 总参考词。比如 `don't`→`don,t`，`3.5`→`3,5`；不做词干或数字归一化。 |
| mixed English WER | 混合语言额外采用 `[a-z0-9_]+` 的 ASCII 词／数字投影，中文不参与；明确不能代表全段 WER。 |
| 缺失／失败与对齐 | 有参考的缺失条目用空 hypothesis，失败用真实 partial 或空值，留在 CER/WER 分母；缺参考条目无法对齐，registered 与 collected 同时列出。全部未调用时 rate=null、state=NOT_RUN。 |
| 专名正例 | NFKC+casefold 精确匹配，ASCII 两侧词界；不接受音译。positive 必须确实出现在逐字稿。分母保留所有有参考预登记正例，未执行不得算命中。task=asr 看识别稿，其他 task 看成稿，应分别解读。 |
| 专名负例 | negative 必须不在逐字稿中；统计实际成功输出的误插率，并同时列预登记检查数与 unobservedTermChecks。缺失／失败不会伪装为负例通过。 |
| 可用成稿 | pipeline/rewrite 的 succeeded、有双人一致／裁决、全部事实和 Mode 条件 correct、无新增事实；pipeline 还要求 protocolComplete=true。分母是全部预登记条目，缺失失败和缺评分保留。task=asr 不宣称有成稿。 |
| 关键事实确认 | 已确认 correct 的 critical 项÷有参考的全部登记 critical 项；缺评分或分歧不算正确。它是已确认比例，不能将尚未评分直接解释为模型必定错误。 |
| 协议 complete | 独立报告，不能代替语义完整或成稿质量。 |
| 延迟 | 成功 primary 的实际测量，毫秒；speech→caption、stop→ASR、stop→Ready、stop→dispatch 分开。P50/P95 用 nearest-rank，即排序后第 ceil(p×n) 个；显示 n、未测登记数；n<20 标 smallSample，不推广人口 P95。失败保留总体分母，但成功延迟不伪造超时测量。 |
| 成本 | 所有已执行 primary 和 diagnostic 均进入 allCallsCost。缺失费用是 unknown，不是0；有缺失或多币种时 total=null，另外显示 knownSubtotals。单组另报每分钟原音频和每条可用成稿成本。费用/分钟按整段冻结音频时长，不裁静音；不是供应商计费规则。未执行条目不算已发生调用。 |

本版为了易审查采用普通 Levenshtein 动态规划，长段大矩阵需预留本机评分时间，评分耗时不进入模型延迟。没有默认95%通过门槛，也不以小样本零严重错误推出生产零错误。新模型切换仍需要冻结真人集、真实停止链路与单独的输入/撤销验收。

## 验证

```powershell
python scripts/voice-quality-test.py
```

边界测试覆盖缺失/失败分母、重复 primary、诊断复跑隔离与全成本、输入/结果漂移、严格 schema、PCM 规格/截断、说话人分离、synthetic/human 分组、混合词界、双人分歧/裁决、不完整 Mode 评分、协议与语义分离、unknown 成本、小样本 P95、默认无正文/路径，以及24条 missing 模板。测试生成的 WAV 是临时 synthetic 静音，只用于格式校验；本轮没有采集或上传真人音频。
