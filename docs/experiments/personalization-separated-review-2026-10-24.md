# 个性化基础组路由与分离复核实验 · 2026-10-24

本轮处理真实基础组的一致性，并检验“内容核对与偏好适用性分开”是否值得继续研究。复核误拒的根因尚未确定；不靠扩大 prompt 或重复调已暴露材料宣布质量提升。

## 基础组的实际问题与修复

原个性化对照把带画像的 `Some(WritingContext)` 克隆后清空画像，两组始终进入 minimum 管线。正常关闭画像时，旧 light、structure 或自定义模式在跟随口述语言、关闭输入上下文的设置下返回 `None`，由已有 provider/mode 路径处理。非阿里配置可因此从单请求变成生成加复核；阿里旧模式的纯删除放行路径也不同。所以“基础整理”不总是等同于实际关闭画像的行为。

普通录音和对照现在共用可选上下文路由。对照独立按关闭画像生成基础上下文，保留语言、输入上下文开关、场合与自动模式规则；没有捕获的宿主文字，不能宣称复现带真实宿主上下文的录音。前端明确说明同源、同场合、同语言及试写不读取其他应用内容。两组保持相同服务、词典、完整来源和各自 20 秒期限，取消基础组不启动画像组。

回归包括 216 组合与正常 no-host freeze 对比、本地 HTTP 单／双请求路径及来源保持、可选基础组取消。请求数是脚本服务收到的次数，不是实际模型成本或真实语音延迟。不同路径是真实开关效果的一部分，不能把输出或耗时差单独归因为职业字段；已有受控文本消融仍单独保留。

## 冻结的 8 次复核预检

来源为 PEBD003 第三轮 D 的完整 source、mode、draft 对象及冻结 WritingContext，不只复用正文；原正文四组相同，但 UUID 和完整对象 hash 原先不同。新 fixture `personalization-preference-review.dev.json` 的 SHA256 为 `3e20ad2cf4d0408f81ad122dd4f0f0a654e3e32bb92f4918a929d354f44d69a8`。来源回执 SHA256 为 `02aed957943ebdc0060c3fe1d35015a3980f6ca6f62bfe158791aa0dbde38bd6`。入口强制核验 fixture 字节以及原 source／draft／context 绑定。

2 × 2 × 2 因子，总上限 8 次，每格一次：

- 协议：显式 named 五项对照；分开内容五项与逐规则适用性／执行状态的 named 候选。对照不是当前 Aliyun 默认 ordered 格式，不混淆两个变量。
- 画像：完整 D；只删除冻结快照中 `instructions` 规则。职业、受众、任务、其他规则、版本和身份均相同，不重新冻结或补预算。
- 草稿：完整原稿；只将“只有收到供应商发票后我才提交报销申请”改为“我会提交报销申请”。其余正文、段落、draft ID、sourceRefs 不变，负例明确丢失发票前提且语法仍完整。

两臂均 Aliyun 北京／qwen3.8-flash，512 输出上限、20 秒单次实验期限、无重试、无跳转、无生成或修复。按固定稿对交替先后顺序。`--plan` 不读凭据或请求网络；`--run` 必须与原计划逐值相同并先写持久账本，再读取凭据。评估标签、原回执路径及 good/bad 名称只进入本地账本，不进入模型请求。

候选保留同一完整来源、场景及 Email 合同，内容五项仍须全通过。个人偏好另外逐 key 返回适用、冲突忽略、不相关或不确定及执行状态；缺失、重复、未知 key 或不一致组合无效。偏好不能豁免内容失败；综合实验接受还须 native admission 与适用偏好通过。默认复核构建与选择不变。

判读分别记录原生守卫、模型内容结论、模型偏好结论。原生拒绝不能掩盖模型放过缺失稿；无有效响应的格子为 NOT_EVALUATED，不能记为正确拒绝。完整稿应内容通过；缺失稿应 coverage 或 scope 发现损失。候选未在这两项发现条件损失时立即停余下调用；任一解析、超时、凭据、持久记录错误也停止，不补跑。

完整稿是已暴露诊断案例，不是新 holdout。若 named 对照也判对，只能报告未复现，不能声称候选修复误拒。即使八格均符合预期，也不足以推广默认、证明职业收益或真人采用；仍需独立材料、普通期限和用户任务。

预检为 `output/personalization-separated-review-20261024.json.plan.json`。实际 token usage 与费用只有供应商收据才能证明，512 是上限而非实际用量。

## 实际运行：按规则在首个错误停止

运行回执 `output/personalization-separated-review-20261024.json` 为 HALTED，`haltReason=review_error_no_retry`，客户端授权请求为 **1/8**。首格为完整 D + 正例 + named 对照，返回 `invalid_semantic_review`，消息为“语义核对问题说明为空或过长”。其余 **7/8 为 NOT_RUN**，包括全部分离候选及条件缺失负例；没有重试、生成或修复。

该错误来自既有严格引文说明校验，不是一个有效模型语义拒绝。共享传输没有保留原始无效响应，无法区分空说明、超长说明或还原其五项结论；不能据此确定模型误拒原因。首格评价为 NOT_EVALUATED，不能算作正确拒绝、错误放行或质量失败率。候选没有实测效果证据，未改变默认选择。本轮不追加调用或放宽 parser 来获得通过结果。

完整计划、逐请求账本与运行日志分别为同名 `.json.plan.json`、`.json.jsonl` 和 `.log`。账本的响应收到状态只表明传输返回，不代表通过解析、语义合格、供应商计费或用户可用。

独立核验确认预检等于 JSONL 首条计划、回执等于 JSONL 最终记录，各输入和源码哈希一致；1 条 started 与 1 条 finished 平衡，传输为 responseReceived，耗时 4881.6182 毫秒。这是单次固定稿复核的传输观察，不是普通 5 秒策略或语音端到端验收。没有完成配对比较，不能计算增量耗时或质量收益。回执 SHA256 为 `af27927a0a322f6ba993ee41e663f79cfcc163b08c7554dd2ccba089cc888125`。

执行顺序也留档：首轮全库回归尚未确认结果时就启动了这次云端实验。全库当时有 1 项旧断言误报；后续未增加云端请求，修正测试并完整重验。这是执行顺序问题，不能将后续通过结果回填成请求前已全库通过。

## 工程验证

| 检查 | 结果 | 仓库外证据及边界 |
| --- | --- | --- |
| 完整 Rust library 重验 | 1038 passed，0 failed，4 ignored | `output/personalization-separated-rechecked-tests.log`，脚本退出 0、缓存 11.797 GiB |
| 实验模块聚焦检查 | 9/9 PASS | `output/personalization-separated-lab-cloud-final-tests.log`，脚本服务／本地解析，不是实际模型质量 |
| 冻结入口 | 4/4 PASS | `output/personalization-separated-runner-final-tests.log`，对象绑定、仅一处改动、标签隔离、全局限额和无效评价 |
| TypeScript / Vite | PASS | `output/personalization-baseline-routing-frontend-build.log` |
| 实际 Edge / React | 19/19 PASS | `output/personalization-baseline-routing-browser-20261024/receipt.json`；合成 IPC，页面错误和外部请求均为 0 |

首轮全库为 1037 passed、1 failed、4 ignored，证据 `output/personalization-separated-final-tests.log` 保留。旧隐私断言把整份事件 JSON 转字符串后搜索源文金额“3000”；计时浮点数也可能偶然含该数字序列。这不能证明内容泄露。改为校验序列化字段白名单、阶段／事件／结局字符串域及数值类型后完整重验，没有放宽对来源／画像文本字段的排除。

本轮清理仅限已检查的过时 0.2.31 debug library 两个文件，释放 164,634,176 字节，收据为 `output/personalization-separated-cache-cleanup.json`；保留当前 0.2.32、既有发布制品、来源和日志，没有递归删除。

模型误拒、普通生产期限下的收益、真实录音及六任务结果仍未完成。基础组路由修复是实际行为修正，分离复核仍只是未获效果证明的显式实验。

## 修复版候选 r2

`output/releases/0.2.32-personalization-2026-10-24-r2/`：

| 文件 | 字节数 | SHA256 |
| --- | --- | --- |
| SpeakMode_0.2.32_x64-setup.exe | 15,155,568 | DD298EB9100178E87D45C10A4746620DEA2B3CB5C0D1B4699C3B20D00ACA7685 |
| SpeakMode.exe | 28,932,608 | BF64180224B50B0C7C0C441EA24EA81EB96C4947CBB367E2C6BD76B63C97CB36 |

manifest 绑定源码 commit `4fcd728089bf03f6fc6d172396039033dc8b4613`、app tree `45de20cb29832cca1265eadd7f7061ff1ae64bef`。`output/personalization-package-routing-0.2.32-r2.log` 确认前端、MSVC release、NSIS 完成，退出 0、缓存 11.798 GiB、无预算违规。EXE 产品版本 0.2.32，复制后逐文件哈希一致。未安装或启动，默认复核器未改变。

版本仍为桌面 0.2.32／前端 0.4.29／schema 5。首次运行的数据库兼容边界与 r1 相同：旧版不能打开已升级数据库。原 r1 制品及试用说明保留，六任务说明改指 r2；已有任务不要求重录。
