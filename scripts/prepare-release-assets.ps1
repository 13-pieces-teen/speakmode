#requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-fA-F]{40}$')][string]$SourceCommit
)
$ErrorActionPreference = 'Stop'
$speakRepo = Split-Path -Parent $PSScriptRoot
$speakConfig = Get-Content (Join-Path $speakRepo 'app/src-tauri/tauri.conf.json') -Raw | ConvertFrom-Json
$speakVersion = $speakConfig.version
$speakName = "SpeakMode_${speakVersion}_x64-setup.exe"
$speakInstaller = Get-Item -LiteralPath $Installer
if ($speakInstaller.Name -cne $speakName -or $speakInstaller.Length -eq 0) {
    throw "Expected a nonempty $speakName installer."
}
if (Test-Path -LiteralPath $OutputDirectory) {
    throw 'Use a new output directory to avoid mixing release assets.'
}
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
Copy-Item -LiteralPath $speakInstaller.FullName -Destination (Join-Path $OutputDirectory $speakName)
Copy-Item -LiteralPath $speakInstaller.FullName -Destination (Join-Path $OutputDirectory 'SpeakMode-windows-x64-setup.exe')
$speakHash = (Get-FileHash -LiteralPath $speakInstaller.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
@("$speakHash  $speakName", "$speakHash  SpeakMode-windows-x64-setup.exe") |
    Set-Content (Join-Path $OutputDirectory 'SHA256SUMS.txt') -Encoding ascii
[ordered]@{
    version = $speakVersion
    sourceCommit = $SourceCommit
    target = 'x86_64-pc-windows-msvc'
    installer = $speakName
    bytes = $speakInstaller.Length
    sha256 = $speakHash
} | ConvertTo-Json | Set-Content (Join-Path $OutputDirectory 'build-info.json') -Encoding utf8
$speakNotesPath = Join-Path $speakRepo "docs/releases/v$speakVersion.md"
if (Test-Path -LiteralPath $speakNotesPath) {
    $speakNotes = Get-Content -LiteralPath $speakNotesPath -Raw
} else {
    $speakNotes = "SpeakMode $speakVersion — Windows x64。更新内容见对应标签的提交记录。"
}
$speakNotes += @"


## 下载与安装

- Windows x64：下载 SpeakMode-windows-x64-setup.exe，或带版本号的同内容安装包。
- 首次使用需要联网准备 WebView2 Runtime，并在应用中配置自己的识别与整理服务 API Key。
- 当前安装包没有 Windows 代码签名，Windows 可能显示发布者未知或 SmartScreen 提示。
- SHA256SUMS.txt 提供文件校验值；build-info.json 记录对应源码提交。
- 当前仅提供 Windows x64 安装包。应用内自动更新尚未接入，升级请下载新版安装包。

源码提交：$SourceCommit
"@
$speakNotes | Set-Content (Join-Path $OutputDirectory 'RELEASE-NOTES.md') -Encoding utf8
