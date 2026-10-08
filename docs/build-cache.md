# Windows 构建与缓存管理

桌面检查、库测试和 NSIS 打包统一使用 `scripts/build-desktop.ps1`。准备好 Windows MSVC、SDK、Rust 和 Node 环境，并在 `app/` 执行一次 `npm ci` 后，从仓库根目录运行：

```powershell
# 只读查看路径、命令和预算，不创建目录、不启动编译。
.\scripts\build-desktop.ps1 -Task Check -ValidateOnly

.\scripts\build-desktop.ps1 -Task Check
.\scripts\build-desktop.ps1 -Task Test
.\scripts\build-desktop.ps1 -Task Package
```

三个任务都使用 `x86_64-pc-windows-msvc` 和 `--locked`。Test 保留桌面库测试及默认 4 个测试线程；Package 调用本地锁定版本的 Tauri CLI，先执行已有前端构建，再生成 NSIS 安装包。不会启动应用或安装器。完整 MSVC 环境是否可用仍由实际编译验证，预检只检查所需命令和本地 CLI 是否存在。

## 固定一份编译缓存

目标目录按以下优先级选择：显式 `-TargetDirectory` → 当前进程的 `CARGO_TARGET_DIR` → `app/src-tauri/target`。相对路径相对仓库根解析；目录名称须为 `target` 或 `target-<说明>`。拒绝路径中或缓存内部的符号链接／junction，避免统计错卷或误用其他目录。

不同版本、实验和 worktree 应复用同一个绝对路径，不复制整份 target：

```powershell
.\scripts\build-desktop.ps1 -Task Test -TargetDirectory 'E:\BuildCache\SpeakMode\target-msvc'
```

更换磁盘时只选择一处长期位置；旧缓存经审查后清理，不把每轮版本号写入缓存路径。发行 EXE、安装包及构建回执可以按版本单独归档，整个 target 不属于发行成果。直接启动开发热更新时也使用这个相同目录和 MSVC target；先以 Check / ValidateOnly 检查预算。

脚本仅在当前调用期间设置 `CARGO_TARGET_DIR`、`CARGO_INCREMENTAL=0`，并把较新 Cargo 支持的 `CARGO_BUILD_BUILD_DIR` 设为同一目录，避免中间产物另起一份缓存。退出时恢复原值，不修改系统／用户环境变量、Rust 工具链或个人运行配置。原 `npm run desktop:build` 保留兼容，但没有这层预算检查；日常检查、测试和打包应使用上面的统一入口。

## 体积和磁盘预算

- target 的逻辑文件大小默认上限 **12 GiB**；NTFS 压缩后的磁盘占用可能较小。
- target、源码、临时文件和显式 `CARGO_HOME` 所在卷，启动前各至少有 **8 GiB** 可用空间。
- 超限拒绝启动，结束后再次统计并报告增长量。检查失败会保留成果，要求先审查缓存再继续。
- `-MaxCacheGiB`、`-MinFreeGiB` 可按明确需要调整；提高阈值不等于已解决磁盘增长。

这是启动前和结束后的检查，**不是操作系统硬配额，也不会在运行中终止编译**。一次冷构建、并行任务或其他软件仍可能消耗预留空间。脚本不会自动删除缓存、下载包、工具链、历史数据库、录音、API Key 或任何用户目录。不要并行启动多轮打包，也不要为绕过检查临时新建多个 target。

## Rust 配置的取舍

`dev` 和 `test` 统一使用 `debug=1`、`incremental=false`；保留有限调试信息和源码位置，避免反复写入增量快照。构建脚本及 proc macro 使用 `build-override.debug=0`，与 Cargo 默认偏向不生成构建依赖调试信息的方向一致。`release` 的 `strip=true`、`lto="thin"`、`codegen-units=1` 保持原值。

关闭增量编译会让部分重复构建变慢，但已有依赖产物仍可复用。需要完整变量／类型调试时可在受控诊断中临时覆盖配置，用完回到此配置；不要长期并存多套调试参数对应的大缓存。修改 profile 后首次构建会重新生成受影响的产物，体积下降比例和速度变化需以真实构建回执衡量，不能从配置直接推算。

参考 [Cargo profiles](https://doc.rust-lang.org/cargo/reference/profiles.html) 与 [Cargo 环境变量](https://doc.rust-lang.org/cargo/reference/environment-variables.html)。

## 清理边界

优先审查废弃 target 和旧增量目录；运行中的编译任务结束后，确认目标绝对路径及归属，再用同一 PowerShell 会话清理。保留正在交付的程序／安装器、源代码、锁文件、第三方打包材料和个人数据。清理 registry 下载缓存或 SDK 会增加下一次下载／工具链准备成本，应单独决定。脚本不提供隐式清理或递归删除入口。
