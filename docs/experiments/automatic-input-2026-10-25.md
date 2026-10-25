# 自动输入判定与个性化验收补充（2026-10-25）

用户症状：微信、浏览器及其他应用存在自动输入没有发生，而手动 Ctrl+V 可以。自动适配继续属于原有直接输入流程，没有新增兼容模式、应用选择或用户开关。

## 修复

旧的未发布窗口适配方案使用 GetLastInputInfo 时间及指针坐标；悬停移动和停止手势松开也可能使其失效。现在复用原有快捷键线程已安装的鼠标／键盘监听，只存操作计数与监听代际，不存按键内容、坐标或历史，不另开全局监听器。普通移动和松开不计为新的输入操作；点击、滚动及按键按下使旧快照失效。鼠标仍按住、监听器停止／重建、多个所有者或心跳超过一秒均不接受旧状态。自身菜单遮罩和一次粘贴批次有既有事件标记，不计为用户输入。

普通精确光标路径优先。只有该路径明确返回能力不支持时，才尝试自动键盘路径：同一已冻结窗口／进程生命周期／焦点句柄，UIA 中同一 Edit／Document／Group 运行时对象，明确启用、可聚焦、持有焦点、非密码且可写。ValuePattern 可写但不提供 TextPattern 的控件也可投递。未知只读属性不放行，不把按钮当输入框；不调用 Value 取值、GetText 或 SetValue。窗口级微信例外仍限制为实际观察的进程及窗口族。

固定检查线程、期限、完整性级别、菜单路由、复制前后验证、350ms 修饰键等待、持久化单次尝试与取消门控保持有效。输入只派发一个 Ctrl+V，任何部分派发或未确认都不自动重试；回执仍诚实标记 unconfirmed。没有精确光标时无法证明应用内部程序性光标变化不存在，这一边界保留。

## 工程检查

- 最终 Rust library：**1061 passed、0 failed、4 ignored**，43.00 秒；包含同一生产操作计数代码的悬停／松开／点击／滚动／键入／代际与失效用例，以及值控件运行时身份、取消、重复尝试及复制期间变更。证据 `output/automatic-input-tests-20261025-final.log`。
- 缓存 11.903 GiB，未提升 12 GiB 门槛，构建预算无违规。没有清理此前被审批拦截的缓存文件。
- 试点核算 Python：8 项合成边界测试通过；记录 `output/personalization-pilot-tests-20261025.log`。工程测试不能代替用户偏好、模型质量或真实录音。
- 输入反馈与真实恢复组件合成检查 21/21、个性化辅助检查 11/11、TypeScript 检查通过。旧输入反馈脚本缺少当前 recorder 的 Fragment／场合目录／尺寸 hook 接口，最初未运行或报错；本轮更新测试接口并加载实际场合目录，最终日志为 `output/delivery-feedback-helper-20261025-r4.log`，旧失败日志保留，没有改写产品行为来让检查通过。

尝试从根包对非 workspace 的 handy-keys 依赖执行单元测试被 Cargo 拒绝，未运行；保留 `automatic-input-hook-tests-20261025.log`。随后在根 library 的测试配置直接包含同一生产计数源码执行其两个测试，纳入上述 1061 项，不把未执行的依赖测试说成通过。

## Windows 原生自建控件验证

`scripts/owned-value-editor.ps1` 编译独立测试应用：它明确提供可写的 ValuePattern，不提供 TextPattern。测试者通过正常窗口按钮启动 `keyboard_delivery_trial`；后者调用生产的捕获、验证和一次投递函数，不转移焦点、不重试，也不访问用户应用、数据库、录音、凭据或网络。操作和读回由 computer-use 的窗口 API 完成。

| 自建案例 | 旧精确路径 | 新路径／原生回执 | 窗口实际读回 |
| --- | --- | --- | --- |
| 可写 ValuePattern-only 控件 | `targetCapabilityUnsupported` | 4 个事件，重复许可 0 个事件 | 中文、英文、ADHD 和数字日期完整插入；插入计数为 1 |
| 同一控件只读 | `targetCapabilityUnsupported` | `targetNotEditable`，0 个事件，未改剪贴板 | 原内容不变，计数为 0 |
| 同一控件标记密码 | `targetNotEditable` | `targetNotEditable`，0 个事件，未改剪贴板 | 原内容不变，计数为 0 |

原始回执及三份独立窗口读回保存在 `output/automatic-input-owned-20261025/`。生产回执未被改写成 confirmed；独立窗口读回只证明这些自建控件的结果。实际微信、浏览器、其他用户应用和真实语音端到端仍为待验收。

测试自身的失败同样保留：第一版测试应用的 64 位 WM_GETOBJECT 参数转为 Int32 时溢出，已在测试程序中修正；其后一次可写控件因 provider 在错误线程读取 Control.Focused 而被拒绝（0 输入、未改剪贴板）。修正为在控件线程报告焦点后复测通过，未放宽产品的焦点检查。旧失败 JSON 不覆盖。Microsoft 的 [服务器 provider 示例](https://learn.microsoft.com/en-us/windows/win32/winauto/uiauto-howto-expose-serverside-uiautomation-provider) 和 [托管返回接口](https://learn.microsoft.com/en-us/dotnet/api/system.windows.automation.provider.automationinteropprovider.returnrawelementprovider) 用于实现测试控件。

## 个性化评估

见 [PRD 验收报告](personalization-acceptance-2026-10-25.md)。当前反馈为定性信息，六任务模板保持 NOT_RUN，所有未测门槛保持 UNKNOWN；本次实际插入测试不计入真人质量任务或模型提升。8 万字符组件耗时的两组失败继续保留，不重新抽样以取得通过。本轮没有追加收费模型请求。
