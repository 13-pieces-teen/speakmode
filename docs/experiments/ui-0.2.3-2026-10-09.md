# SpeakMode 0.2.3：标题字体与用量验证

2026-10-09。基线为已合并 0.2.2（`c658678`，PR #3），本版桌面 0.2.3／前端 0.4.3，工作分支 `fix/usage-typography-0.2.3`。范围见[版本计划](../plans/ui-0.2.3.md)。本机证据根目录：`D:/Codex_Project/SpeakMode_Dev/evidence/ui-0.2.3-2026-10-09-r1`。

**当前状态：前端与修复后的 Rust 全库验证通过；Windows 程序与 NSIS 已构建并核对。** 应用与安装器未启动，原始失败记录保留。

## 问题与实际改动

字体元数据确认，本地 Noto Sans SC 变量字体只有 553 个字符。旧版生产构建的逐字 CDP 结果显示，合成标题中“藏”“筛”“算”使用 Microsoft YaHei UI，其余部分中文使用 Noto Sans SC，证明问题发生在实际字形选择，不能靠比较 `font-family` 字符串发现。基线证据保存在 `font-baseline/actual-fonts.json` 与截图中。

本版通过 `unicode-range` 将已有本地字体限定为拉丁字符，常用中文统一使用 Windows 系统字体。品牌、页面、卡片、记录、Mode、词典和设置标题共用字体与 600 字重 token，原字号层级保持；不下载新字体。新版采样的中文标题逐字实际字体均为 Microsoft YaHei UI，混排问题在这些样本中消除。

用量此前已经持久化。原首页只显示本机当日，日期变化后归零是原有统计口径；本版保留默认今日，增加具体日期及累计切换。页面独立读取数据库统计，不再把默认零值当作已读结果；加载显示占位，失败显示重试，同范围刷新失败保留上次值。累计通过只读 `usage.total` 汇总已有 `usage_events`，不新增 schema 迁移，也不把个人历史正文重新用于计数。

日期更新覆盖页面重新激活、可见性变化与可见页面每 30 秒检查。审查发现过一个边界：午夜前发起请求、午夜后返回新日数据时，结果曾沿用请求起点的昨日标签；已改为使用经过校验的响应日期，并分别测试新日与旧日迟到响应。

## 浏览器与前端结果

TypeScript／Vite 构建通过。新增脚本 `scripts/usage-typography-browser-qa.py` 使用生产 React、隔离的合成 Tauri bridge 与 headless Edge 154；检查通过不代表读取了真实用户数据库。

| 范围 | 结果 | 证据 |
| --- | --- | --- |
| 标题实际字体 | 中文逐字平台字体一致；品牌保留本地拉丁字体 | `font-baseline`、`browser-green-r2` |
| 新增字体／用量 UI | **15/15 PASS** | `browser-green-r2/receipt.json` |
| 既有 UI 回归 | **14/14 PASS** | `ui-regression-r1/receipt.json` |
| TS／Vite | PASS | `frontend-build.log` |

新增检查覆盖四种尺寸 1440×900、1024×768、720×640、430×720；页面重载后重新读取、当日为空但累计保留、加载非零假象、失败重试、刷新失败保留旧值、范围切换乱序、跨日定时刷新、跨日回到窗口，以及午夜前后两种响应。首轮 13/13 通过后补充两个午夜响应用例，最终 15/15 通过；不把两轮重复项累加成新的通过数量。

旧 UI 14 项回归覆盖长预览、完整复制请求、五列导航、历史独立滚动与加载、详情焦点、延迟取消 ACK、设置反馈、多个个人 Mode 与词典焦点。两组浏览器最终运行均无页面错误，预览服务已退出。430px 是补充浏览器布局检查，原生最小宽度仍为 720px。

## Rust 失败与最终复验

第一次 Windows desktop 全库结果为 **466 passed／1 failed／3 ignored**。失败项为 `usage_total_ipc_is_a_main_window_read_with_camel_case_counters`：新增累计读取遗漏了成功后 `changed` 广播的排除表，使只读命令推动事件序号。该排除已补齐；这是真实发现的行为缺陷，不能把首轮记录写成全绿。

第二次编译在 D 盘空间不足时退出 101，错误为 `rustc-LLVM ERROR: IO failure on output stream: no space on device`，未获得修复后的全库结果。`rust-tests.log`、`rust-tests-r2.log` 与各自收据保留。

最终改用 `E:/Codex_Build/SpeakMode/ui-0.2.3-2026-10-09` 的独立源码镜像复验，未执行旧缓存删除。Windows desktop 全库 **467 passed／0 failed／3 ignored**。10 个关键 Rust/config 文件在编译前后与 canonical SHA256 一致。新增测试覆盖真实隔离 SQLite 文件两次关闭重开、跨日、删除历史、不保留历史、重复提交不重计、未完成不计，以及只读 IPC 契约。

最终原生证据位于上述 E 盘目录下的 `evidence/rust-tests-r3.log`、`rust-tests-r3-receipt.json` 和 `rust-mirror-inputs-before/after.json`。

## Windows 分发与边界

Windows x64 MSVC release／NSIS 构建退出 0，179 项构建输入前后相同，14 件冻结分发资源核对通过。额外的 `canonical-source-check.json` 确认全部 179 项输入与 D 盘 canonical 源码一致。程序和安装包已独立复制到 `E:/Codex_Build/SpeakMode/ui-0.2.3-2026-10-09/release`，ProductVersion／FileVersion 均为 0.2.3，复制前后 SHA-256 一致：

| 文件 | SHA-256 |
| --- | --- |
| SpeakMode.exe | `9e938e3187e136f49f50cb8c34cf9e76956a5449c0554488d6dd7d0ca20d5aca` |
| SpeakMode_0.2.3_x64-setup.exe | `7bc3eb896bdadd1a1ef7b133db46dc4486f968056861c4dcaf0413e2245df1c6` |

交付目录附带 `Start-SpeakMode.cmd`／`.ps1`、`build-receipt.json` 和 `handoff-receipt.json`；启动器语法检查通过，应用和安装器均未启动，运行中的旧版本未被关闭，个人配置未改变。启动器沿用已有资料目录，检测到另一版本仍在运行时拒绝重复启动。由于原资料所在 D 盘本轮发生空间不足，实际试录前应释放空间；E 盘分发路径不代表资料目录已迁移。

本轮不更改云端模型、提示词、采音、自动输入或旧用量写入规则。浏览器使用合成数据，原生测试使用隔离测试数据库；没有真实麦克风、云端模型或宿主输入验收。个人配置、正文、原始数据库和实际个人用量不进入仓库。真实桌面 DPI、安装／升级与用户口述仍需独立验收。
