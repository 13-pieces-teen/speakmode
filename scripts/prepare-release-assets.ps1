#requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Installer,
    [Parameter(Mandatory)][string]$OutputDirectory,
    [Parameter(Mandatory)][ValidatePattern('^[0-9a-fA-F]{40}$')][string]$SourceCommit,
    [string]$Signature,
    [string]$UpdaterPublicKey,
    [switch]$RequireUpdater,
    [ValidatePattern('^[A-Za-z0-9][A-Za-z0-9_.-]*/[A-Za-z0-9][A-Za-z0-9_.-]*$')]
    [string]$Repository = '13-pieces-teen/speakmode'
)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$speakRepo = Split-Path -Parent $PSScriptRoot
$OutputDirectory = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
$speakConfig = Get-Content (Join-Path $speakRepo 'app/src-tauri/tauri.conf.json') -Raw -Encoding utf8 | ConvertFrom-Json
$speakVersion = $speakConfig.version
if ($speakVersion -isnot [string] -or $speakVersion -notmatch '^\d+\.\d+\.\d+$') {
    throw 'Release assets require a stable desktop version string.'
}
$speakName = "SpeakMode_${speakVersion}_x64-setup.exe"
$speakInstaller = Get-Item -LiteralPath $Installer
if ($speakInstaller -isnot [IO.FileInfo] -or $speakInstaller.Name -cne $speakName -or $speakInstaller.Length -eq 0) {
    throw "Expected a nonempty $speakName installer."
}
if (Test-Path -LiteralPath $OutputDirectory) {
    throw 'Use a new output directory to avoid mixing release assets.'
}
$speakUpdater = $RequireUpdater -or -not [string]::IsNullOrWhiteSpace($Signature)
$speakSignatureFile = $null
$speakSignature = $null
if ($speakUpdater) {
    if ([string]::IsNullOrWhiteSpace($Signature)) { $Signature = $speakInstaller.FullName + '.sig' }
    $speakSignatureFile = Get-Item -LiteralPath $Signature
    if ($speakSignatureFile -isnot [IO.FileInfo] -or $speakSignatureFile.Name -cne "$speakName.sig" -or
        $speakSignatureFile.Length -eq 0 -or $speakSignatureFile.Length -gt 65536) {
        throw "Expected a nonempty, bounded $speakName.sig updater signature."
    }
    $speakSignature = (Get-Content -LiteralPath $speakSignatureFile.FullName -Raw -Encoding utf8).Trim()
    try { $speakSignatureBytes = [Convert]::FromBase64String($speakSignature) } catch {
        throw 'Updater signature must contain the base64 text produced by Tauri signing.'
    }
    if ($speakSignatureBytes.Length -eq 0) { throw 'Updater signature is empty.' }
    if ([Convert]::ToBase64String($speakSignatureBytes) -cne $speakSignature) {
        throw 'Updater signature must be a single canonical base64 value.'
    }
    try {
        $speakUtf8 = [Text.UTF8Encoding]::new($false, $true)
        $speakSignatureLines = @($speakUtf8.GetString($speakSignatureBytes).TrimEnd([char[]]"`r`n") -split '\r?\n')
        if ($speakSignatureLines.Count -ne 4 -or -not $speakSignatureLines[0].StartsWith('untrusted comment: ') -or
            -not $speakSignatureLines[2].StartsWith('trusted comment: ')) { throw 'Invalid Minisign signature structure.' }
        $speakSignaturePacket = [Convert]::FromBase64String($speakSignatureLines[1])
        $speakGlobalSignature = [Convert]::FromBase64String($speakSignatureLines[3])
        if ($speakSignaturePacket.Length -ne 74 -or $speakGlobalSignature.Length -ne 64 -or
            [Text.Encoding]::ASCII.GetString($speakSignaturePacket, 0, 2) -cnotin @('Ed', 'ED')) { throw 'Invalid Minisign signature packet.' }
        $speakTrustedFields = $speakSignatureLines[2].Substring('trusted comment: '.Length) -split "`t"
        $speakSignedVersions = @($speakTrustedFields | Where-Object { $_.StartsWith('version:') })
        $speakSignedFiles = @($speakTrustedFields | Where-Object { $_.StartsWith('file:') })
        if ($speakSignedVersions.Count -ne 1 -or $speakSignedVersions[0] -cne "version:$speakVersion" -or
            $speakSignedFiles.Count -ne 1 -or $speakSignedFiles[0] -cne "file:$speakName") {
            throw 'Updater signature must bind this exact version and installer filename.'
        }
    } catch { throw "Invalid updater signature: $($_.Exception.Message)" }
    if ([string]::IsNullOrWhiteSpace($UpdaterPublicKey)) { throw 'Signed assets require UpdaterPublicKey content.' }
    try {
        $speakPublicKeyBytes = [Convert]::FromBase64String($UpdaterPublicKey.Trim())
        $speakPublicKeyLines = @($speakUtf8.GetString($speakPublicKeyBytes).TrimEnd([char[]]"`r`n") -split '\r?\n')
        if ($speakPublicKeyLines.Count -ne 2 -or -not $speakPublicKeyLines[0].StartsWith('untrusted comment: ')) {
            throw 'Invalid Minisign public-key structure.'
        }
        $speakPublicKeyPacket = [Convert]::FromBase64String($speakPublicKeyLines[1])
        if ($speakPublicKeyPacket.Length -ne 42 -or [Text.Encoding]::ASCII.GetString($speakPublicKeyPacket, 0, 2) -cne 'Ed') {
            throw 'Invalid Minisign public-key packet.'
        }
        if ([Convert]::ToBase64String([byte[]]$speakSignaturePacket[2..9]) -cne
            [Convert]::ToBase64String([byte[]]$speakPublicKeyPacket[2..9])) { throw 'Updater signature and configured public key have different key IDs.' }
    } catch { throw "Invalid updater public key: $($_.Exception.Message)" }
    # These are envelope/version/key-ID checks, not cryptographic verification.
    # Tauri signs the exact artifact in CI; the app verifies both signatures.
}
New-Item -ItemType Directory -Path $OutputDirectory | Out-Null
Copy-Item -LiteralPath $speakInstaller.FullName -Destination (Join-Path $OutputDirectory $speakName)
Copy-Item -LiteralPath $speakInstaller.FullName -Destination (Join-Path $OutputDirectory 'SpeakMode-windows-x64-setup.exe')
$speakHash = (Get-FileHash -LiteralPath $speakInstaller.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
$speakChecksums = @("$speakHash  $speakName", "$speakHash  SpeakMode-windows-x64-setup.exe")
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
    $speakNotes = Get-Content -LiteralPath $speakNotesPath -Raw -Encoding utf8
} else {
    $speakNotes = "SpeakMode $speakVersion — Windows x64。更新内容见对应标签的提交记录。"
}
$speakUpdateNote = '本地未签名构建不提供自动更新清单，升级请下载新版安装包。'
if ($speakUpdater) {
    $speakUpdateNote = '支持应用内更新的版本可检查并安装签名更新；此前没有更新功能的版本需先手动安装一次。'
}
$speakNotes += @"


## 下载与安装

- Windows x64：下载 SpeakMode-windows-x64-setup.exe，或带版本号的同内容安装包。
- 首次使用需要联网准备 WebView2 Runtime，并在应用中配置自己的识别与整理服务 API Key。
- 当前安装包没有 Windows 代码签名，Windows 可能显示发布者未知或 SmartScreen 提示。
- SHA256SUMS.txt 提供文件校验值；build-info.json 记录对应源码提交。
- 当前仅提供 Windows x64 安装包。$speakUpdateNote

源码提交：$SourceCommit
"@
$speakNotes | Set-Content (Join-Path $OutputDirectory 'RELEASE-NOTES.md') -Encoding utf8
if ($speakUpdater) {
    $speakSignatureName = "$speakName.sig"
    Copy-Item -LiteralPath $speakSignatureFile.FullName -Destination (Join-Path $OutputDirectory $speakSignatureName)
    $speakManifest = [ordered]@{
        version = $speakVersion
        notes = $speakNotes
        pub_date = [DateTime]::UtcNow.ToString('o')
        platforms = [ordered]@{
            'windows-x86_64' = [ordered]@{
                signature = $speakSignature
                url = "https://github.com/$Repository/releases/download/v$speakVersion/$speakName"
            }
        }
    }
    # JSON consumed by the updater must be UTF-8 without a BOM, including PS5.1.
    [IO.File]::WriteAllText((Join-Path $OutputDirectory 'latest.json'),
        ($speakManifest | ConvertTo-Json -Depth 6), [Text.UTF8Encoding]::new($false))
    foreach ($speakExtra in @($speakSignatureName, 'latest.json')) {
        $speakExtraHash = (Get-FileHash -LiteralPath (Join-Path $OutputDirectory $speakExtra) -Algorithm SHA256).Hash.ToLowerInvariant()
        $speakChecksums += "$speakExtraHash  $speakExtra"
    }
}
$speakChecksums | Set-Content (Join-Path $OutputDirectory 'SHA256SUMS.txt') -Encoding ascii
