# SpeakMode 0.2.34 整合与交付记录

2026-11-05。桌面 0.2.34，前端 0.4.31。整合本轮精简交互与墨蓝浮条设计，并清理已确认失效的代码。基线为 `d2de816`，工作分支 `fix/dictation-reliability-20261028`。

## 当前产品行为

录音浮条只显示取消、状态和结束。墨蓝底色与浅色结束按钮构成主要操作层级，中间显示 7 条真实音量采样；录音为“正在聆听”，处理为“正在转成文字”。基础浮条 274×48，原生基础窗口 298×68。字幕默认关闭，可在设置开启；场合菜单继续从托盘进入。

统一状态映射用于浮条和主窗口。收起异常提示只隐藏窗口，保留内容；原生再次检查会话 ID 和代次，阻止旧请求隐藏新会话。查看结果按会话定位并遵守未保存编辑保护。正常未确认派发不显示“已填入”，也不自动重试输入。

结果页保留正文、复制和精确纠词作为主要操作，其他操作与本次参考按需展开。记入词典默认不勾选，现有 Unicode 选区、版本冲突和事务保持不变。完整识别校验、整理期限、原文回退及一次性投递策略沿用既有实现。

## 冗余代码整理

| 清理项 | 依据与处理 |
| --- | --- |
| 旧语义复核模块 | 删除 `rewrite_semantic_review_v1.rs`；没有模块声明或可执行调用，唯一外部引用是历史来源哈希记录。历史实现仍可从 Git 获取。 |
| 多余包装入口 | 删除未被调用的 `rewrite_manual_candidate_with_writing_context`、`generate_with_writing_context`，保留实际使用的带执行期限入口。 |
| 浮条旧样式 | 删除旧模式触发器、分隔线、投递卡片、消息卡片、结果链接和参考注释共 6 组失效选择器，保留当前字幕、菜单、恢复和减少动态效果样式。 |
| 仅供测试的辅助函数 | 将传输替换、试验复核证据、同步 checkpoint/commit 和快照辅助入口限定到 `cfg(test)`，继续运行对应回归测试。 |
| 导入与导出 | 删除未使用图标及重复测试导入；个性化模块的通配导出收敛为 3 个实际对外使用的类型。 |
| 热键重复常量 | Windows hook 与注入事件统一使用现有 `STANDALONE_MENU_MASK_EXTRA_INFO`。数值仍为 `0x484B_4D4D`，去掉不可达的重复匹配分支；同步本地修改说明及分发材料哈希。 |
| 版本入口 | README、Cargo、Tauri、前端 package 与 lock 文件统一为本次版本；此前报告保留为历史证据。 |

没有把“编译器报告未使用”直接等同于可删除：个性化通配导出在首次清理后触发私有导入编译错误，已改为显式类型导出。首次失败日志保留，最终结果以随后完整检查为准。

## 构建与检查

状态：最终检查与 EXE/NSIS 打包通过。使用原有 MSVC 构建适配器及唯一 `cache/target-msvc-formal`，本次调用缓存预算为 13 GiB，没有改动默认策略。

| 检查 | 最终结果 |
| --- | --- |
| TypeScript（含未使用声明/参数检查） | PASS |
| 生产 Vite 构建 | PASS；保留既有依赖的模块指令及大 chunk 提示 |
| Rust 默认库测试 | 1086 passed / 0 failed / 4 ignored，46.72 秒；本次测试编译无警告 |
| 呈现/投递、字幕、声波、纠词、恢复选区 helper | 27 + 8 + 7 + 5 + 11 = 58/58 PASS |
| 安装器分发材料 | 14 件逐一核对字节数和 SHA-256，PASS |
| EXE/NSIS | 构建退出 0，两个文件的 ProductVersion 均为 0.2.34；安装包复制哈希一致 |
| 构建输入冻结 | 358 个产品/分发输入在构建前后哈希一致 |

最终安装包为 `SpeakMode_0.2.34_x64-setup.exe`，15,194,385 bytes，SHA-256：`591494f95b6009aaf9b20b9927c58bd73264d6eddd0fb8fda2a936a4b2c9f858`。构建结束时缓存 12.530 GiB，预算检查 `violations=[]`；随后的旧安装文件归档会减少 target 内的文件，但不释放整个磁盘的空间。程序和安装器未启动或安装，不能由打包结果推断原生验收完成。

```powershell
npm.cmd --prefix app run check
# 本轮额外核对没有未使用的本地声明/参数。
app/node_modules/.bin/tsc.cmd -p app/tsconfig.json --noEmit --noUnusedLocals --noUnusedParameters
powershell -NoProfile -File D:\Codex_Project\SpeakMode_Dev\scripts\build-speakmode.ps1 -Task Test -MaxCacheGiB 13
powershell -NoProfile -File D:\Codex_Project\SpeakMode_Dev\scripts\build-speakmode.ps1 -Task Package -MaxCacheGiB 13
```

## 安装文件保留规则

新包为 0.2.34，保留已有 0.2.33 和 0.2.32 安装文件作为回退。清理范围是工作区内 0.2.31 及更早、文件名严格匹配 `SpeakMode_<version>_x64-setup.exe` 的 77 个安装文件，共 526.14 MiB；每项已记录版本、绝对路径、字节数和 SHA-256。

永久删除被自动审批审查以 `blocked by policy` 拒绝，没有进一步原因，删除未执行。随后采用可逆归档：77 个文件已移入 `output/archived-installers/2026-11-05/`，完整保留原工作区相对路径，并核对移动后的 SHA-256；0 失败。旧包已移出当前交付/缓存目录，**释放空间为 0 bytes**，不能称为已永久删除。`installer-cleanup.json` 包含原路径、归档位置与恢复依据。对应源码、历史报告、收据和用户资料未删除。

本机验证记录目录：`reports/release-integration-2026-11-05/`。最终安装包目录：`output/releases/0.2.34-integrated-2026-11-05/`。这些目录位于开发工作区，不进入源码提交。提交采用明确文件清单，现有未跟踪 `validation/` 不纳入。

## 验收边界

本次不将测试、生产构建或安装包生成等同于真实口述验收。麦克风、云端识别、跨应用输入、多屏/DPI、现有资料升级、安装/卸载与用户完成时间保持各自的 `NOT_RUN` 状态，除非下文有本次新增的实际证据。未调整模型或等待预算，不宣称端到端提速。
