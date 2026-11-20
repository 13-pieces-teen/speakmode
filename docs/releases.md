# 发布 Windows 安装包

用户入口：[最新 Release](https://github.com/13-pieces-teen/speakmode/releases/latest) · [直接下载 Windows x64](https://github.com/13-pieces-teen/speakmode/releases/latest/download/SpeakMode-windows-x64-setup.exe)。

## 发布新版本

1. 更新 `app/src-tauri/tauri.conf.json`、`app/src-tauri/Cargo.toml` 及 Cargo.lock 中的桌面版本。`app/package.json` 是独立的前端包版本，不作为 Release 标签依据。
2. 在 `docs/releases/v版本号.md` 写更新说明与实际验证范围，将源码和发布说明合入 main。
3. 在待发布提交打标签并推送，例如：

```powershell
git switch main
git pull --ff-only
git tag -a v0.2.39 -m "SpeakMode 0.2.39"
git push origin v0.2.39
```

`.github/workflows/release.yml` 校验版本与签名配置，运行桌面库测试、前端构建及 NSIS 打包，创建草稿、上传后发布为 latest。仅接受 `v数字.数字.数字` 稳定版本；手动运行也必须选择版本标签。不要把示例版本当作当前发布命令执行。

签名流水线还需要以下 GitHub Actions 配置；缺少公钥或私钥会在构建前失败，不会退回未签名发布：

- Repository variable `TAURI_UPDATER_PUBLIC_KEY`：Tauri 生成的完整 base64 公钥内容，不是文件路径。
- Repository secret `TAURI_SIGNING_PRIVATE_KEY`：与公钥配套的私钥内容。
- Repository secret `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`：私钥密码；未加密密钥可留空。不要将私钥、密码放入仓库或发布资产。

在受控位置备份签名密钥与密码。后续版本沿用同一对密钥，已安装客户端才能用内置公钥验签。

基础 `tauri.conf.json` 保留 HTTPS 更新端点及 `requireSignedVersion` 策略，不提交公钥。CI 在 runner 临时目录生成最小配置覆盖，仅合并 `bundle.createUpdaterArtifacts=true` 和 `plugins.updater.pubkey`，通过 `build-desktop.ps1 -Task Package -TauriConfig <配置文件>` 传给 Tauri。此参数也接受 JSON 对象文本；相对配置文件路径按仓库根目录解析，参数必须在 Cargo 分隔符 `--` 之前。普通本地构建不传覆盖时仍可生成手动安装包。

每次签名 Release 提供以下资产：

- `SpeakMode_版本_x64-setup.exe` 与固定名 `SpeakMode-windows-x64-setup.exe`，内容相同。
- `SpeakMode_版本_x64-setup.exe.sig`：版本安装包对应的 Tauri 签名。
- `latest.json`：`windows-x86_64` 平台、签名文件内容、版本、UTC 发布时间及说明。下载 URL 固定到该版本的标签和文件名，不使用会随 Latest 改变的安装包 URL。
- `SHA256SUMS.txt` 与 `build-info.json`：保留原手动安装包校验值、字节数与源码提交；校验清单还包括签名和更新清单。

`prepare-release-assets.ps1 -RequireUpdater -UpdaterPublicKey <base64公钥内容>` 默认读取安装包旁的同名 `.sig`，也可用 `-Signature <路径>` 指定。签名缺失、格式异常、可信注释缺少匹配版本/文件名，或签名与配置公钥的 key ID 不一致，都会拒绝准备资产。`-Repository owner/repo` 默认本仓库，CI 使用当前 GitHub 仓库名生成固定标签 URL。脚本的检查不等于密码学验签；Tauri 签名器负责实际签名，客户端再次验证完整签名与可信版本。当前 CLI 2.12.1 的 `tauri build` 会自动写入签名可信注释中的版本；手工签名必须显式传 `signer sign --app-version 版本`。

固定名使 README 下载入口无需随版本修改。安装包不提交到 Git。首次 0.2.38 使用已有本地构建，元数据记录实际构建源码提交；后续标签由 Actions 构建。仅准备本地手动包时不传 `-RequireUpdater` 或 `-Signature`，不会生成可被自动更新端点发现的清单。

失败时先看 Actions 日志。所有安装包、签名、更新清单与元数据先上传同一个草稿，全部成功后才提升为 Latest。上传失败会保留草稿；检查并处理草稿后再重试，流程不会覆盖既有 Release 或安装包。已发布版本需要修正时使用新的补丁版本，保留历史下载。发布后检查 Release 为非草稿、非预发布，下载文件的 SHA256 与清单一致，且 `latest.json` 的版本、固定标签 URL 与签名文件内容相符。

更新签名与 Windows 发布者代码签名是两回事。安装包尚未配置 Windows 代码签名，仍可能显示未知发布者；使用语音服务仍需自备 API Key，首次准备 WebView2 需要网络。没有更新功能的旧版本需先手动安装一次含更新功能的版本。原生使用、签名校验及安装升级验收独立于构建成功，按各版本说明记录。

## 不签名、不构建的流水线回归

```powershell
powershell.exe -NoProfile -File scripts/test-release-assets.ps1
pwsh.exe -NoProfile -File scripts/test-release-assets.ps1
```

测试在新的 `.validation/release-assets-fixtures-*` 目录创建合成安装包和 Minisign 形状数据，检查清单/校验值、可信版本、公钥 key ID、无签名拒绝和配置参数顺序。构建包装器仅以 `ValidateOnly` 调用，并使用不能执行的假工具文件；不会运行 Cargo、Node、真实签名、安装器或远程发布。它不验证真实密钥、密码学签名或客户端升级。
