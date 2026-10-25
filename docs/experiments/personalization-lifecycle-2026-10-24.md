# 个性化生命周期与证据审计 · 2026-10-24

本轮完成删除后的前端缓存清理、重启后的未完成任务收尾和复核协议记录修复。没有追加云模型请求，没有安装或启动原生程序。P0 质量门槛仍未通过，PR 保持 Draft。

## 删除参考详情后不再由缓存恢复

原后端已删除 SQLite 参考侧表，但前端复用历史详情缓存；自动刷新主要覆盖最近三条或正文修订号变化的记录。删除画像不会改变正文修订号，因此较早打开的第四条及之后历史仍可能显示旧画像。

`DesktopProvider` 现在只根据成功的原生配置回执推进版本和身份 ID 策略，清理全部缓存、列表、临时结果及会话参考；策略不额外保留完整画像。迟到的历史读取和快照在接受时重新检查，历史页本地副本及对照结果在显示时也检查。旧版本配置回执不能恢复旧身份。普通编辑、停用和失败保存不会清除仍有效的历史参考。

清理只影响参考字段：背景、名称、规则及省略详情变为空并显示清除标记；正文、来源、画像版本标识及修订 ID 保留。此处不宣称 JavaScript 堆或磁盘取证级擦除。

验证包括 11 项生产组件配合合成 hooks/IPC 的检查，以及 2 项实际 Edge/React 浏览器回归。浏览器故意让模拟后端在删除后仍返回完整旧参考，分别检查第四条缓存和删除前发起、删除后返回的历史读取。删除前后的正文、sourceRevisionId 和 outputRevisionId 相同，旧背景哨兵消失，清除标记可见。已查看删除后的截图。

浏览器中的复制探针被合成 IPC 拒绝，仅用于读取组件提交的修订身份并展示手动复制内容；不访问系统剪贴板。浏览器回归证明前端处理，不替代原生后端删除验证或真实用户试用。

## 重启后不再永久显示进行中

数据库所有者已在启动、接受新任务之前调用 `recover_after_restart`。现在同一事务也将有效的旧 `Started` 画像事件改为既有 `Failed`，不冒充成功或用户主动取消，不把停机时间当处理耗时。

已完成任务及显式反馈、配置、来源和正文不变。普通 `Store::open` 或事件读取不触发收尾，避免中断仍有所有者的任务；旧任务的迟到成功不能覆盖恢复后的终态。损坏的统计行保持原样并在统计读取时报告错误，不伪造有效记录，也不阻断普通会话恢复。

真实 SQLite 回归覆盖：所有三种任务、既有成功及反馈、回退和取消的保持、重复恢复幂等、事务失败回滚、损坏统计的保持。另通过真实 DB worker 关闭／重开验证启动收尾，以及新所有者期间普通读取不会收尾活跃任务；这不是强制杀进程或原生 GUI 重启测试。

## 复核记录区分两个协议

原有 `protocolVersion` 在写作上下文路径上记录场合合同，覆盖了底层复核协议。新增只由本地准备请求决定的 `reviewerWireProtocolVersion`，在初次及最终复核中保留实际选用的五项核对协议。例如：

```json
{"protocolVersion":"situational-dictation-v4","reviewerWireProtocolVersion":"minimum-review-ordered-v1"}
```

该字段记录请求选择的合同，不声称模型响应一定是数组：ordered 路径仍兼容原有 named 响应。模型不得返回这些版本字段，响应 schema、接受规则、源文绑定、期限和请求数没有放宽。该变化提升诊断可追溯性，不修复模型误拒本身。

## 验证证据

所有日志在仓库外的 `D:\Codex_Project\SpeakMode_Dev\output\`。

| 检查 | 结果 | 证据与边界 |
| --- | --- | --- |
| 完整 Rust library | 1003 passed，0 failed，4 ignored | `personalization-lifecycle-final-tests.log`；新增 actor 测试加入前的完整快照 |
| 新增实际 DB worker 重启回归 | 1 passed | `personalization-lifecycle-owner-restart-test.log`；其余 1007 项未在这次过滤运行中执行，不能称为再次全库通过 |
| 消融／覆盖实验入口 | 10/10、2/2 PASS | `personalization-lifecycle-quality-example-tests.log`、`personalization-lifecycle-coverage-example-tests.log`；仅离线工具回归 |
| 前端配置 helper | 11/11 PASS | `personalization-lifecycle-helper-check.log` |
| 新增组件及缓存回归 | 11/11 PASS | `personalization-reference-component-check.log`；使用合成 hooks/IPC，不是真实 React 浏览器 |
| 原有个性化浏览器回归 | 17/17 PASS | `personalization-lifecycle-browser-20261024/receipt.json`；实际 Edge，合成 IPC，无外部请求、pageErrors 为 0 |
| 新增删除缓存浏览器回归 | 2/2 PASS | `personalization-reference-browser-20261024-r1/receipt.json`；实际 Edge，模拟旧后端参考，无外部请求、pageErrors 为 0 |
| TypeScript / Vite | PASS | `personalization-lifecycle-frontend-build.log` |

供手动试用的 r4 已完成前端／MSVC／NSIS 构建，退出 0，未安装或启动。制品、SHA256 与源码绑定见 [实施记录](personalization-0.2.31-2026-10-23.md)；该构建不改变下列未完成项。

## 尚未完成的 P0

后续修订：下文的实际发送口径缺陷已在 0.2.32 中修复，详见 [请求统计实施](personalization-dispatch-2026-10-24.md)。本节保留 r4 审计时的发现；它不表示新版本仍以快照选择计数。模型误拒与真人质量验收仍未完成。

“已参考画像”当前依据冻结快照是否存在，而不是含画像请求是否真正开始发送。普通／预览任务可以在本地校验或请求前取消时已经记录 true；对照也可能在基础组取消，从未启动画像组。故这些计数不能作为实际已发送画像请求或质量收益证据。既有事件也无法凭当前 true 值可靠追溯到供应商收件。

后续需区分选中快照、客户端请求提交发送、供应商收件或计费，按内容无关的真实发送边界计数；保持同一任务去重、画像版本和历史反馈，未核验的旧事件单列。现有 timing observer 在供应商转换之前启动，不能直接当成实际发送证明。这个口径缺陷本轮只完成定位，尚未修复，不据此将统计验收记为完成。

模型复核仍会误拒完整成稿；六个真实录音任务、目标应用交付、用户采纳与修改负担也尚无结果。P1 建议式学习没有被当作 P0 必需项扩充。工程修复和浏览器检查不能替代上述验收。
