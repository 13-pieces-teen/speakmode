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

`.github/workflows/release.yml` 校验版本，运行桌面库测试、前端构建及 NSIS 打包，创建草稿、上传后发布为 latest。仅接受 `v数字.数字.数字` 稳定版本；手动运行也必须选择版本标签。不要把示例版本当作当前发布命令执行。

每次 Release 提供带版本号的安装包、固定名安装包（内容相同）、SHA256SUMS.txt 和 build-info.json。固定名使 README 下载入口无需随版本修改。安装包不提交到 Git。首次 0.2.38 使用已有本地构建，元数据记录实际构建源码提交；后续标签由 Actions 构建。

失败时先看 Actions 日志。上传失败会保留草稿；检查并处理草稿后再重试，流程不会覆盖既有 Release 或安装包。已发布版本需要修正时使用新的补丁版本，保留历史下载。发布后检查 Release 为非草稿、非预发布，且下载文件的 SHA256 与清单一致。

当前为未签名的 Windows x64 安装包，需要自备云服务 API Key；尚未配置应用内自动更新。首次准备 WebView2 需要网络。原生使用与安装升级验收独立于构建成功，按各版本说明记录。
