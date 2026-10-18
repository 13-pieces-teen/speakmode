# 0.2.25：引导恢复与紧凑录音浮层

日期：2026-10-18。基线：0.2.24 / frontend 0.4.23；本版：0.2.25 / 0.4.24。

## 实际问题与修改

- 用户的文档练习结果元数据显示 `timeout`。短文本原先只有 5 秒，完整生成、复核可能来不及完成。引导是 Preview，现使用 20 秒总上限；日常 5/8/20 秒策略保持原有行为。
- 原文回退与“成稿已就绪 / 复制成稿”矛盾。现统一显示识别原文、明确原因，并提供“重试整理，无需重录”。重试复用原 source 和 base revision，通过已有候选与采用接口保存；有复核疑点时保留原文。继续按钮要求确认原文，原生端校验精确 output revision。
- 多段输出曾溢出结果框并覆盖下方提示。现限制在结果框内滚动，提示、复制和录音控件保持独立。
- 浮层从 396×48 工具栏收紧到 212×44 深色胶囊：左侧取消，中间真实音量波形与场合入口，右侧结束。无文字时原生窗口为 240×64；有字幕、错误或交付信息时宽 344，高度仍受 420 上限约束。场合名称可从 title、辅助名称及菜单读取。
- 长字幕、错误、时限和长场合列表同时出现时，菜单入场位移会让旧的几何判定持续失败，造成隐藏等待。现检查 Popper wrapper 的最终布局边界，入场仅移动 2px；键盘、托盘和 reduced motion 即时显示。选择后立即收起菜单，标签仍等待原生确认。
- 原生窗口调整移至 blocking worker，避免占用异步处理线程。当前目录中的场合快照直接复用，历史个人 revision 保留存储读取路径。每个新 session 重新同步尺寸，DPI/work-area 底部锚点与输入目标校验保持有效。

## 验证结果

| 检查 | 结果 | 范围 |
| --- | --- | --- |
| Windows MSVC 原生回归 | PASS：853 / 0 failed / 4 ignored | AppHub、SQLite、取消、版本绑定、场合冻结、原文确认、预算和几何；其中新增的云端探针默认 ignored |
| 固定文档、真实云端整理与复核 | PASS：1/1，2,923 ms | 使用已配置服务；一条固定合成文本，不读取个人历史文本，不采集麦克风，不输入宿主 |
| 引导和浮层界面行为 | PASS：22/22 | 正式 React 组件 + 合成 IPC，含 1220×820 与 720×560、失败重试、精确原文确认、结束出口、键盘、reduced motion、长字幕/23 选项/错误/时限组合 |
| 模拟 350ms 原生尺寸及场合应答 | 选择后菜单 12.9ms 收起；最大并发 resize=1 | 单次合成测量，不能推广为真实桌面延迟保证 |
| 类型检查、生产前端、MSVC EXE/NSIS | PASS | 复用单一缓存；安装包未自动安装 |
| 实际麦克风、Windows 焦点回交与宿主输入 | NOT_RUN | 本轮未更换用户正在运行的程序 |

真实云端输出：

```text
今天同步三件事：
1. 登录页已经完成。
2. 支付页还在联调，预计周五完成。
3. 发布前先补一轮回归测试，通过以后再上线。
```

来源未变，保留了“预计”和发布前置条件。一次请求不能代表后续网络稳定性或全部识别质量。

浏览器证据同时保留了开发刷新时的 8 条历史 MutationObserver 异常，其来源未取得堆栈；清洁父、子页面重载未再产生 Runtime 异常。本轮不声称所有开发期控制台记录均无错误。另有一次被 HMR 重置打断的检查，已另记并重跑，不计入 22 项行为分母。

## 重现

`scripts/compact-recorder-browser-lab.html` 和相邻 `compact-recorder-browser-fixture.html` 是可控的浏览器尺寸模拟器，不随产品打包。复制两者到 `app/.validation/recorder/`，从 `app/` 启动 Vite 后打开 lab；其中按钮可模拟 350ms 延迟、长字幕、长菜单、提示、托盘入口及重置。

引导夹具为 `scripts/onboarding-browser-fixture.html`，同样复制到 `app/.validation/`。`?qaStep=document` 仅为合成练习跳转；可用 QA 按钮模拟超时与复核失败，并查看已脱敏的命令摘要。`recorder-mode-browser-qa.py` 尺寸断言已同步并通过语法检查，本轮的浏览器执行使用上述夹具与 CUA，未运行旧 Python 驱动。

真实云端探针为默认跳过的 `onboarding_live_document_probe`。仅在明确选择服务及账单后设置 `SPEAKMODE_PROBE_CONFIG_DB`、新的 `SPEAKMODE_PROBE_OUTPUT`，再执行 `cargo test --features desktop --lib onboarding_live_document_probe -- --ignored --test-threads=1`。探针只读 provider_config，凭据留在原生凭据实现中；创建独占回执后，最多处理一条固定文本，不自动重试。

本机证据目录：`output/validation/onboarding-recorder-2026-10-18/`，含原生日志、云端回执、浏览器逐项记录、截图和最终包构建日志。证据、个人资料、构建缓存和安装包均不进入源码提交。
