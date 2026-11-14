# 第一条录音完成后快捷键失效：0.2.41

用户反馈：第一次录音结束后，再按快捷键无法开始下一条。本机运行版本为 0.2.40，实际设置为 F8；只读数据库元数据显示最近一条口述为 Ready，输入回执为 Unconfirmed / inputDispatchedUnconfirmed。输入派发回执不证明宿主已接受文字，也不能单独证明本次故障根因。

## 代码确认的问题

`ShortcutTap` 为防止组合键误触发，保存所有已按下的普通键和鼠标键。旧 Windows listener 只校正修饰键，没有为普通键补发漏失的释放；任何孤立 down 都可能一直留在 `held` 中，使后续 F8 的 `held.len() != 1` 持续成立而被拒绝。

另有一条确定的遗漏路径：观察流在修饰键按住时报告左/右鼠标 down；如果先松修饰键再松鼠标，旧 listener 不报告该 mouse up。该路径会留下无法自行清除的鼠标按住状态。其他 hook 拦截、桌面切换或键盘映射变化也可能造成缺失/不匹配的 release。本轮没有取得故障瞬间的物理事件记录，不能断言用户本次必然由其中某一种动作触发。

## 修改

- Windows hook owner 按虚拟键维护已观察到的普通键/鼠标键。原生消息排空后，通过 `GetAsyncKeyState` 只校正已松开的键，每轮空闲等待上限沿用 10ms。
- 补偿释放标记为 Reconciled，清除旧手势但不触发 Toggle；实际仍按住的键继续保留。该检查不在 hook 内运行，以免把尚未更新异步状态的当前 down 误判为 up。
- 观察流始终报告左/右鼠标的成对事件；非观察 API 保持原有按修饰键过滤的策略。
- 同一虚拟键的 release 使用 down 时记录的 Key，避免按住期间键盘映射改变留下旧 Key。
- 左/右鼠标键的补偿等待两个物理主按钮都松开，兼容交换左右键或按住期间改变映射；真实 mouse up 仍立即平衡原 down。
- 不向系统注入补偿按键；原有 injected-event 过滤、长按/组合键约束、会话资源释放和一次性输入门禁继续生效。

## 验证

- Windows MSVC 桌面库：1105 passed，0 failed，4 ignored。新增覆盖：漏普通键 up 后连续十次 F8、漏 F8 up 后必须重新实按、正常/跳过/失败派发结束后的下一条录音准入。
- vendored handy-keys 库：97 passed，0 failed。新增覆盖：漏普通键/鼠标键释放只补一次 Reconciled、仍按住的 F8 不清除、映射变化仍平衡原 down、左右鼠标键交换时仍保留按住状态。
- TypeScript 检查通过；前端与安装包构建结果另记本机发行回执。

测试使用合成事件/状态、隔离 SQLite 和本机 HTTP；不把自动注入按键当作物理快捷键验收。界面检查被用户 Esc 中断后未继续操作界面。本轮未验收真实麦克风、云端请求、连续物理 F8、宿主输入或安装升级。运行中的原应用未被替换或重启。

实现依据：[低级键盘 hook 在异步按键状态更新前调用](https://learn.microsoft.com/en-us/windows/win32/winmsg/lowlevelkeyboardproc)、[异步状态的高位和物理鼠标按钮语义](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-getasynckeystate)。这是 API 行为依据，不代表本机物理验收。

本机日志：工作区 `output/validation/hotkey-repeat-tests-20261114-r4.log`、`hotkey-vendor-tests-20261114-r4.log`。此前编译/命令失败日志保留，以上成功日志为最终测试依据。
