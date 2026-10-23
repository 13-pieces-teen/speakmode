# 固定稿逐单元覆盖复核 · 2026-10-24

状态：24 次固定稿复核已完成。候选与基线的最终判定完全相同，均拒绝 6/6 份缺失稿、接受 5/6 份完整稿，并误拒同一份等义改写；候选在全部 12 对中更慢。本轮没有支持切换生产默认复核器的收益证据。

## 方法与冻结边界

使用 Aliyun / `qwen3.8-flash`，6 个开发案例各有预先固定的 good/bad 草稿，两臂分别复核同一草稿，共 6 × 2 × 2 = 24 次请求。没有生成新草稿、局部修复或真实录音。每个固定稿对内交替安排两臂顺序；预检中 12 对的 source、draft、context 和 unitManifest 哈希分别一致。表达习惯固定为简洁、分段，无职业背景、宿主输入或词典。

- 基线：直接采用实验执行时的生产 minimum reviewer 请求；实际协议均为 `minimum-review-ordered-v1`，输出上限 512 tokens。
- 候选：从同一生产请求追加 `source-unit-coverage-lab-v1`，在**同一次**复核中加入人工冻结的 `sourceUnits` / `draftUnits` 和逐单元对应表，输出上限 768 tokens。保留 coverage / facts / scope / intent / cleanup 五项检查；仅五项通过且全部来源单元为 covered 才能接受。missing / uncertain 不能被五项通过豁免。

单元必须精确拼回冻结全文，包含标点、空格和换行；这只验证分段完整性，不证明分段具有正确语义，也没有实现自动语义分段。good/bad 标签和 criteria 仅供评估，不进入模型请求。实现见 `app/src-tauri/src/cloud/rewrite_coverage_lab.rs:119–202`，配对与顺序见 `app/src-tauri/examples/personalization_coverage_review_lab.rs:37–63`。

实际使用 `fixtures/personalization-coverage-review-v2.dev.json`，SHA256 为 `4f51df75d0e3e9feafe28c6c7cb1a5f1db5e9f1acdaa7b6c4655f0153242ead1`。v2 的 PCOV-001 原文逐字复用已暴露的 PHO-007；good 草稿复用旧 B 组成稿，bad 草稿复用旧 C/D 组成稿。因此它是诊断回放，整套 v2 属于开发材料，不能称为新的独立 holdout。v1 保留原样，SHA256 为 `5b260c4dbd1a262e2982df9637a26beebae6893cd3635b559efd1f55895e25b8`；本轮没有运行 v1。

## 判定结果

| 指标 | 基线 | 候选 |
| --- | ---: | ---: |
| 完成复核 | 12/12 | 12/12 |
| 缺失稿被拒绝 | 6/6 | 6/6 |
| 完整稿被接受 | 5/6 | 5/6 |
| 完整稿误拒 | 1/6 | 1/6 |
| 错误 / 超时 | 0/12 | 0/12 |

六个缺失对象依次为：PCOV-001 地址证明与当天结果提问背景；002 底座裂痕照片；003 社区活动群这一发送目的地；004 培训签到表；005 随物料附上的签收单；006 温湿度探头接线照片。005 的有效改口“周五”和去重后的遥控器、006 的逐字代码在两份草稿中仍保留，缺失判定不依赖删错日期、重复词或代码。

PCOV-004 good 将“培训签到表 / 器材借用单”改为冻结评估准则明确允许的“培训出席记录表 / 设备借用登记表”。两臂仍均返回 coverage=fail、decision=reject；候选额外把来源单元 0 标为 missing。逐单元输出没有解决这个等义改写误拒，也没有在本轮增加任何缺失稿拦截。

## 复核耗时

主计时采用每行 `outcome.elapsedMs`，从实验复核函数开始到解析结束，包含本地准备与专用客户端创建。配对差值先按同一 caseId + draftKind 计算“候选 − 基线”，再取 12 对的中位数；它不等于两臂中位数之差。

| 计时（毫秒） | 基线中位数，n=12 | 候选中位数，n=12 | 配对差值中位数，n=12 |
| --- | ---: | ---: | ---: |
| `outcome.elapsedMs` | 1180.5 | 2147 | +1063.5 |
| `requests[0].durationMs`，请求观察器 | 1176.60415 | 2142.7345 | +1063.55305* |

\* 请求观察器差值显示到小数点后五位；原始 JSON 浮点十进制值直接计算为 `1063.55305000000025`。`elapsedMs` 的配对差值无此浮点尾差。

候选 12/12 对都更慢；`elapsedMs` 配对增量范围为 +103 至 +2344 毫秒。两臂中位数之差为 966.5 毫秒，不能替代 +1063.5 毫秒的配对中位差。本轮同时改变附加协议、请求数据、响应结构和输出上限，不能据此把耗时差单独归因于某一项。只运行一次，未估计重复运行波动；这些值不是普通生成→复核链路或麦克风到目标应用的延迟。

## 账本与可核对材料

最终回执为 COMPLETED，24 行均有一次客户端请求开始且收到响应，haltReason=null；JSONL 有 24 条 arm_started、24 条 arm_completed、24 条请求 started 和 24 条 finished。逐行结果与最终 run_finished 回执和独立 JSON 回执一致；独立 plan 与回执内 plan 一致。自动重试 0、跳转 0，每次实验复核期限 20000 毫秒。请求计数是客户端授权的管线开始次数，不代表供应商收据或计费次数。

原始材料均在仓库外，完整路径如下；本报告没有重跑请求或改写这些材料。

- 最终回执：`D:\Codex_Project\SpeakMode_Dev\output\personalization-coverage-review-20261024.json`
- 预检：`D:\Codex_Project\SpeakMode_Dev\output\personalization-coverage-review-20261024.json.plan.json`
- 逐请求账本：`D:\Codex_Project\SpeakMode_Dev\output\personalization-coverage-review-20261024.json.jsonl`
- 执行日志：`D:\Codex_Project\SpeakMode_Dev\output\personalization-coverage-review-20261024.log`
- 复用样例的旧回执：`D:\Codex_Project\SpeakMode_Dev\output\personalization-holdout-v1-20261023\PHO-007.json`

回执内 executable SHA256：`dcf35da2a8aab7f9825d214d88333aced4f4129d4d10a2d7d5d6218076788b6c`。入口源码 SHA256 为 `95542534f9531cb444e95d2d4b3e392aa3ed33a0b08127edf56e7efa10a8f948`，覆盖模块为 `4fcfc7ff7daeaa35c4674b2e58c28d6cb7bdeeef8bba790751a0cb342b33c60a`，均与报告核对时的对应文件一致；不据此声称其他生产源码已全部绑定或验证。

`actualTokenUsage=null`：共享传输没有暴露响应 usage，因此实际 token 数与费用未知。512 / 768 只是输出上限，不能作为实际用量。

## 解释与后续适用范围

PHO-007 旧 C/D 曾放行该缺失稿，而本轮连基线也拒绝它。这里的画像、复核请求和冻结场景与旧运行不同，没有控制变量隔离，不能声称已确定旧问题的根因或证明候选修复了它。

本轮完成后，用户新增了邮件“直接可发送正文”的契约：允许省略明确面向助手的写作包装，但必须保留其中的材料、客户问题和其他实质内容。它不属于本轮执行契约。本轮 good/bad 仍按冻结时的请求保留规则命名，PCOV-001 good 中保留“请帮我回复”不代表它满足新邮件正文标准；不得用新策略回填旧标签或改写冻结材料。新契约下的评估需要另立版本和输出。

逐单元对应是模型判断证据，属于 `MODEL_ALIGNMENT_NOT_SEMANTIC_PROOF`。本轮未测试真实录音、自动分段、目标应用输入、用户采纳或修改负担；audio / userQuality 均为 NOT_RUN，deliveryAuthorized=false。候选只保留为显式实验入口，不据此推广到生产默认路径，也不声称语义保真、用户质量或生产性能获得提升。
