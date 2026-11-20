#requires -Version 5.1
<#
.SYNOPSIS
Exercise release preparation and Package ValidateOnly with synthetic fixtures.
.DESCRIPTION
Never runs Cargo, Node, signing, installers or GitHub writes. Synthetic Minisign
envelopes are deliberately not cryptographic signatures. Fixtures are retained.
#>
[CmdletBinding()]
param([string]$OutputDirectory)
Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutputDirectory)) {
    $OutputDirectory = Join-Path $repo ('.validation/release-assets-fixtures-' + [Guid]::NewGuid().ToString('N'))
}
$OutputDirectory = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutputDirectory)
if (Test-Path -LiteralPath $OutputDirectory) { throw 'Use a fresh fixture directory.' }
$fixture = Join-Path $OutputDirectory 'repo'
$fixtureScripts = Join-Path $fixture 'scripts'
$fixtureTauri = Join-Path $fixture 'app/src-tauri'
$fixtureCli = Join-Path $fixture 'app/node_modules/@tauri-apps/cli'
$null = New-Item -ItemType Directory -Path $fixtureScripts, $fixtureTauri, $fixtureCli -Force
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'prepare-release-assets.ps1') -Destination $fixtureScripts
Copy-Item -LiteralPath (Join-Path $PSScriptRoot 'build-desktop.ps1') -Destination $fixtureScripts
$prepare = Join-Path $fixtureScripts 'prepare-release-assets.ps1'
$wrapper = Join-Path $fixtureScripts 'build-desktop.ps1'
$version = '0.0.123'
$name = 'SpeakMode_' + $version + '_x64-setup.exe'
$installer = Join-Path $OutputDirectory $name
$signaturePath = $installer + '.sig'
$configPath = Join-Path $fixtureTauri 'tauri.conf.json'
$utf8 = [Text.UTF8Encoding]::new($false)
[IO.File]::WriteAllText($configPath, '{"version":"0.0.123"}', $utf8)
[IO.File]::WriteAllText((Join-Path $fixtureTauri 'Cargo.toml'), '[package]', $utf8)
[IO.File]::WriteAllText((Join-Path $fixtureCli 'tauri.js'), 'fixture only; never execute', $utf8)
[IO.File]::WriteAllBytes($installer, [byte[]]@(77, 90, 0, 1, 2, 3))
$commit = '0123456789abcdef0123456789abcdef01234567'
$passed = [Collections.Generic.List[string]]::new()
function Assert-Case([bool]$Condition, [string]$Name) {
    if (-not $Condition) { throw "Fixture failed: $Name" }
    $passed.Add($Name)
}
function Assert-Rejected([string]$Name, [scriptblock]$Action) {
    $rejected = $false
    try { & $Action | Out-Null } catch { $rejected = $true }
    Assert-Case $rejected $Name
}
function New-SyntheticPublicKey([byte]$KeyId = 1) {
    $packet = New-Object byte[] 42
    $packet[0] = 69; $packet[1] = 100; $packet[2] = $KeyId
    $text = 'untrusted comment: synthetic test public key' + [Environment]::NewLine +
        [Convert]::ToBase64String($packet) + [Environment]::NewLine
    [Convert]::ToBase64String($utf8.GetBytes($text))
}
function New-SyntheticSignature([string]$SignedVersion, [string]$SignedFile = $name) {
    $packet = New-Object byte[] 74
    $packet[0] = 69; $packet[1] = 68; $packet[2] = 1
    $trusted = 'trusted comment: timestamp:1' + [char]9 + 'file:' + $SignedFile
    if ($SignedVersion) { $trusted += [char]9 + 'version:' + $SignedVersion }
    $text = 'untrusted comment: synthetic test signature' + [Environment]::NewLine +
        [Convert]::ToBase64String($packet) + [Environment]::NewLine +
        $trusted + [Environment]::NewLine +
        [Convert]::ToBase64String((New-Object byte[] 64)) + [Environment]::NewLine
    [Convert]::ToBase64String($utf8.GetBytes($text))
}
$publicKey = New-SyntheticPublicKey
$signature = New-SyntheticSignature $version
$common = @{ Installer = $installer; SourceCommit = $commit }
$signed = @{ RequireUpdater = $true; UpdaterPublicKey = $publicKey }

$manualOutput = Join-Path $OutputDirectory 'manual'
& $prepare @common -OutputDirectory $manualOutput
Assert-Case (-not (Test-Path -LiteralPath (Join-Path $manualOutput 'latest.json'))) 'unsigned local assets omit update manifest'
Assert-Case ((Get-Content -LiteralPath (Join-Path $manualOutput 'SHA256SUMS.txt')).Count -eq 2) 'manual checksum format retained'
$missingOutput = Join-Path $OutputDirectory 'missing-signature'
Assert-Rejected 'required signature cannot fall back to unsigned' { & $prepare @common @signed -OutputDirectory $missingOutput }
Assert-Case (-not (Test-Path -LiteralPath $missingOutput)) 'invalid signed inputs create no output directory'

[IO.File]::WriteAllText($signaturePath, $signature + [Environment]::NewLine, $utf8)
$signedOutput = Join-Path $OutputDirectory 'signed'
& $prepare @common @signed -OutputDirectory $signedOutput
$manifestBytes = [IO.File]::ReadAllBytes((Join-Path $signedOutput 'latest.json'))
$manifestText = $utf8.GetString($manifestBytes)
$manifest = $manifestText | ConvertFrom-Json
$platform = $manifest.platforms.'windows-x86_64'
Assert-Case ($manifest.version -is [string] -and $manifest.version -ceq $version) 'manifest version is exact string'
Assert-Case ($platform.url -ceq "https://github.com/13-pieces-teen/speakmode/releases/download/v$version/$name") 'manifest pins versioned GitHub asset URL'
Assert-Case ($platform.signature -is [string] -and $platform.signature -ceq $signature) 'manifest uses signature content without trailing newline'
Assert-Case (-not ($manifestBytes[0] -eq 239 -and $manifestBytes[1] -eq 187 -and $manifestBytes[2] -eq 191)) 'manifest UTF-8 has no BOM'
$utcMatch = [regex]::Match($manifestText, '"pub_date"\s*:\s*"([^"]+)"')
Assert-Case ($utcMatch.Success -and [DateTimeOffset]::Parse($utcMatch.Groups[1].Value).Offset -eq [TimeSpan]::Zero) 'manifest timestamp is UTC RFC3339'
$originalHash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash.ToLowerInvariant()
foreach ($asset in @($name, 'SpeakMode-windows-x64-setup.exe')) {
    Assert-Case ((Get-FileHash -LiteralPath (Join-Path $signedOutput $asset) -Algorithm SHA256).Hash.ToLowerInvariant() -ceq $originalHash) "installer bytes preserved: $asset"
}
Assert-Case ((Get-FileHash -LiteralPath (Join-Path $signedOutput "$name.sig") -Algorithm SHA256).Hash -ceq
    (Get-FileHash -LiteralPath $signaturePath -Algorithm SHA256).Hash) 'signature file bytes preserved'
$info = Get-Content -LiteralPath (Join-Path $signedOutput 'build-info.json') -Raw | ConvertFrom-Json
Assert-Case ($info.sourceCommit -ceq $commit -and $info.version -ceq $version -and $info.sha256 -ceq $originalHash -and $info.bytes -eq 6) 'manual build metadata preserved'
$checksums = @(Get-Content -LiteralPath (Join-Path $signedOutput 'SHA256SUMS.txt'))
Assert-Case ($checksums.Count -eq 4) 'signed manifest includes updater asset checksums'
foreach ($line in $checksums) {
    if ($line -notmatch '^([0-9a-f]{64})  ([^/\\]+)$') { throw 'Malformed checksum fixture result' }
    Assert-Case ((Get-FileHash -LiteralPath (Join-Path $signedOutput $Matches[2]) -Algorithm SHA256).Hash.ToLowerInvariant() -ceq $Matches[1]) 'published checksum matches bytes'
}
Assert-Rejected 'existing release directory cannot be overwritten' { & $prepare @common @signed -OutputDirectory $signedOutput }
Assert-Rejected 'signature public key is required' { & $prepare @common -RequireUpdater -OutputDirectory (Join-Path $OutputDirectory 'missing-public-key') }
Assert-Rejected 'different signing key ID is rejected' {
    & $prepare @common -RequireUpdater -UpdaterPublicKey (New-SyntheticPublicKey 2) -OutputDirectory (Join-Path $OutputDirectory 'wrong-public-key')
}
foreach ($invalid in @(
    @{ Name = 'empty'; Text = ' ' },
    @{ Name = 'malformed'; Text = 'not base64' },
    @{ Name = 'wrong-version'; Text = (New-SyntheticSignature '0.0.122') },
    @{ Name = 'unversioned'; Text = (New-SyntheticSignature '') },
    @{ Name = 'wrong-file'; Text = (New-SyntheticSignature $version 'other.exe') },
    @{ Name = 'not-minisign'; Text = [Convert]::ToBase64String($utf8.GetBytes('fixture')) }
)) {
    [IO.File]::WriteAllText($signaturePath, $invalid.Text, $utf8)
    Assert-Rejected ("invalid signature rejected: " + $invalid.Name) {
        & $prepare @common @signed -OutputDirectory (Join-Path $OutputDirectory $invalid.Name)
    }
}
[IO.File]::WriteAllText($signaturePath, $signature, $utf8)
Assert-Rejected 'repository cannot inject URL path' {
    & $prepare @common @signed -Repository '../other' -OutputDirectory (Join-Path $OutputDirectory 'invalid-repository')
}
[IO.File]::WriteAllText($configPath, '{"version":123}', $utf8)
Assert-Rejected 'numeric version cannot produce an update manifest' {
    & $prepare @common @signed -OutputDirectory (Join-Path $OutputDirectory 'numeric-version')
}
[IO.File]::WriteAllText($configPath, '{"version":"0.0.123"}', $utf8)

# Fake .exe entries satisfy read-only command discovery. They are invalid PE
# files, so even an accidental execution cannot run Cargo, Node or any payload.
$fakeBin = Join-Path $OutputDirectory 'fake-bin'
$null = New-Item -ItemType Directory -Path $fakeBin
foreach ($tool in @('cargo.exe', 'node.exe')) {
    [IO.File]::WriteAllBytes((Join-Path $fakeBin $tool), [byte[]]@(77, 90))
}
$oldPath = $env:PATH
try {
    $env:PATH = $fakeBin + [IO.Path]::PathSeparator + $oldPath
    $basePlan = @{ Task = 'Package'; ValidateOnly = $true; TargetDirectory = (Join-Path $OutputDirectory 'target-fixture'); MinFreeGiB = 0.001 }
    $plain = (& $wrapper @basePlan) -join [Environment]::NewLine | ConvertFrom-Json
    Assert-Case ($plain.allowed -and '--config' -cnotin $plain.arguments) 'ordinary local Package remains unchanged'
    $override = '{"bundle":{"createUpdaterArtifacts":true},"plugins":{"updater":{"pubkey":"fixture"}}}'
    $plan = (& $wrapper @basePlan -TauriConfig $override) -join [Environment]::NewLine | ConvertFrom-Json
    $configIndex = [Array]::IndexOf([object[]]$plan.arguments, '--config')
    $cargoIndex = [Array]::IndexOf([object[]]$plan.arguments, '--')
    Assert-Case ($configIndex -ge 0 -and $configIndex + 1 -lt $cargoIndex -and $plan.arguments[$configIndex + 1] -ceq $override) 'inline config is one argument before Cargo separator'
    Assert-Case ($plan.arguments[-1] -ceq '--locked') 'Cargo locked flag retained'
    $overridePath = Join-Path $fixture 'override with spaces.json'
    [IO.File]::WriteAllText($overridePath, $override, $utf8)
    $filePlan = (& $wrapper @basePlan -TauriConfig 'override with spaces.json') -join [Environment]::NewLine | ConvertFrom-Json
    Assert-Case ($overridePath -cin $filePlan.arguments) 'relative config file resolves from repository including spaces'
    Assert-Rejected 'non-Package task cannot use Tauri config' { & $wrapper -Task Test -TauriConfig $override -ValidateOnly }
    Assert-Rejected 'malformed JSON config is rejected before build' { & $wrapper @basePlan -TauriConfig '{' }
    [IO.File]::WriteAllText($overridePath, '["not an object"]', $utf8)
    Assert-Rejected 'array config is rejected before build' { & $wrapper @basePlan -TauriConfig $overridePath }
} finally { $env:PATH = $oldPath }
[ordered]@{
    passed = $passed.Count
    failed = 0
    cases = @($passed)
    fixturesDirectory = $OutputDirectory
    actualBuildOrSigningRun = $false
    cryptographicSignaturesVerified = $false
} | ConvertTo-Json -Depth 4
