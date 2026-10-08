#requires -Version 5.1
<#
.SYNOPSIS
Run a Windows desktop check, library test or NSIS build using one bounded cache.
.DESCRIPTION
ValidateOnly performs read-only path, cache and disk checks and prints a JSON
plan. It does not create directories, change environment variables or compile.
Budgets are preflight thresholds, not live disk quotas. Nothing is auto-deleted.
#>
[CmdletBinding()]
param(
    [ValidateSet('Check', 'Test', 'Package')]
    [string]$Task = 'Check',
    [string]$TargetDirectory,
    [ValidateRange(0.001, 1048576)]
    [double]$MaxCacheGiB = 12,
    [ValidateRange(0.001, 1048576)]
    [double]$MinFreeGiB = 8,
    [ValidateRange(1, 64)]
    [int]$TestThreads = 4,
    [switch]$ValidateOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
$speakRepo = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$speakApp = Join-Path $speakRepo 'app'
$speakManifest = Join-Path $speakApp 'src-tauri\Cargo.toml'
$speakTriple = 'x86_64-pc-windows-msvc'
if (-not (Test-Path -LiteralPath $speakManifest -PathType Leaf)) {
    throw 'Run the script from an intact SpeakMode checkout.'
}
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw 'The desktop build entry point requires Windows.'
}

if ([string]::IsNullOrWhiteSpace($TargetDirectory)) {
    $TargetDirectory = $env:CARGO_TARGET_DIR
}
if ([string]::IsNullOrWhiteSpace($TargetDirectory)) {
    $TargetDirectory = Join-Path $speakApp 'src-tauri\target'
}
if (-not [IO.Path]::IsPathRooted($TargetDirectory)) {
    $TargetDirectory = Join-Path $speakRepo $TargetDirectory
}
$speakTarget = [IO.Path]::GetFullPath($TargetDirectory).TrimEnd('\', '/')
# Accept a dedicated cache folder, never a volume root, source or runtime folder.
if ([IO.Path]::GetFileName($speakTarget) -notmatch '^target(?:[-_].+)?$') {
    throw 'TargetDirectory must be a dedicated folder named target or target-<name>.'
}

function Assert-SpeakPlainPath([string]$Path) {
    $speakCursor = [IO.Path]::GetFullPath($Path)
    while ($speakCursor) {
        if (Test-Path -LiteralPath $speakCursor) {
            $speakItem = Get-Item -LiteralPath $speakCursor -Force
            if (-not $speakItem.PSIsContainer) {
                throw "Expected a directory: $speakCursor"
            }
            if (($speakItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                throw "Cache path must not traverse a link or junction: $speakCursor"
            }
        }
        $speakParent = [IO.Directory]::GetParent($speakCursor)
        if ($null -eq $speakParent) { break }
        $speakCursor = $speakParent.FullName
    }
}

function Get-SpeakCacheMeasure([string]$Path) {
    Assert-SpeakPlainPath $Path
    [long]$speakBytes = 0
    [long]$speakFiles = 0
    if (Test-Path -LiteralPath $Path) {
        $speakDirectories = New-Object 'System.Collections.Generic.Stack[string]'
        $speakDirectories.Push($Path)
        while ($speakDirectories.Count -gt 0) {
            $speakDirectory = $speakDirectories.Pop()
            foreach ($speakEntry in [IO.Directory]::EnumerateFileSystemEntries($speakDirectory)) {
                $speakAttributes = [IO.File]::GetAttributes($speakEntry)
                if (($speakAttributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
                    throw "Cache contains a link or junction; inspection stopped: $speakEntry"
                }
                if (($speakAttributes -band [IO.FileAttributes]::Directory) -ne 0) {
                    $speakDirectories.Push($speakEntry)
                } else {
                    $speakBytes += ([IO.FileInfo]$speakEntry).Length
                    $speakFiles++
                }
            }
        }
    }
    [pscustomobject]@{ bytes = $speakBytes; gib = [Math]::Round($speakBytes / 1GB, 3); files = $speakFiles }
}

function Get-SpeakDiskMeasures {
    $speakPaths = @($speakTarget, $speakApp)
    if ($env:TEMP) { $speakPaths += $env:TEMP }
    if ($env:CARGO_HOME) { $speakPaths += $env:CARGO_HOME }
    $speakSeen = @{}
    foreach ($speakPath in $speakPaths) {
        $speakRoot = [IO.Path]::GetPathRoot([IO.Path]::GetFullPath($speakPath))
        if ($speakSeen.ContainsKey($speakRoot)) { continue }
        $speakSeen[$speakRoot] = $true
        $speakDrive = New-Object IO.DriveInfo($speakRoot)
        if (-not $speakDrive.IsReady) { throw "Build drive is not ready: $speakRoot" }
        [pscustomobject]@{
            root = $speakRoot
            freeBytes = $speakDrive.AvailableFreeSpace
            freeGiB = [Math]::Round($speakDrive.AvailableFreeSpace / 1GB, 3)
        }
    }
}

function Get-SpeakBudgetViolations($Cache, $Disks) {
    if ($Cache.bytes -gt ($MaxCacheGiB * 1GB)) {
        "Cache is $($Cache.gib) GiB, above the $MaxCacheGiB GiB preflight limit. Review old build artifacts before retrying."
    }
    foreach ($speakDisk in $Disks) {
        if ($speakDisk.freeBytes -lt ($MinFreeGiB * 1GB)) {
            "Drive $($speakDisk.root) has $($speakDisk.freeGiB) GiB free; at least $MinFreeGiB GiB is required."
        }
    }
}

$speakCache = Get-SpeakCacheMeasure $speakTarget
$speakDisks = @(Get-SpeakDiskMeasures)
$speakViolations = @(Get-SpeakBudgetViolations $speakCache $speakDisks)
$speakCommand = 'cargo.exe'
$speakArguments = @('check', '--locked', '--manifest-path', $speakManifest, '--target', $speakTriple, '--features', 'desktop')
if ($Task -eq 'Test') {
    $speakArguments = @('test', '--locked', '--manifest-path', $speakManifest, '--target', $speakTriple, '--features', 'desktop', '--lib', '--', "--test-threads=$TestThreads")
} elseif ($Task -eq 'Package') {
    $speakCommand = 'node.exe'
    $speakTauri = Join-Path $speakApp 'node_modules\@tauri-apps\cli\tauri.js'
    $speakArguments = @($speakTauri, 'build', '--ci', '--target', $speakTriple, '--bundles', 'nsis', '--', '--locked')
    if (-not (Test-Path -LiteralPath $speakTauri -PathType Leaf)) {
        $speakViolations += 'Local Tauri CLI is missing. Run npm ci in app before packaging.'
    }
}
$speakTool = Get-Command $speakCommand -CommandType Application -ErrorAction SilentlyContinue | Select-Object -First 1
if ($null -eq $speakTool) {
    $speakViolations += "$speakCommand is not on PATH. Prepare the Rust MSVC / Node build environment first."
}
if ($Task -eq 'Package' -and -not (Get-Command cargo.exe -CommandType Application -ErrorAction SilentlyContinue)) {
    $speakViolations += 'cargo.exe is not on PATH. Prepare the Rust MSVC environment before packaging.'
}

$speakPlan = [ordered]@{
    task = $Task
    validateOnly = [bool]$ValidateOnly
    repository = $speakRepo
    workingDirectory = $speakApp
    target = $speakTriple
    targetDirectory = $speakTarget
    cache = $speakCache
    disks = $speakDisks
    maxCacheGiB = $MaxCacheGiB
    minFreeGiB = $MinFreeGiB
    executable = $(if ($null -eq $speakTool) { $speakCommand } else { $speakTool.Source })
    arguments = $speakArguments
    processEnvironment = @{ CARGO_TARGET_DIR = $speakTarget; CARGO_BUILD_BUILD_DIR = $speakTarget; CARGO_INCREMENTAL = '0' }
    violations = $speakViolations
    allowed = ($speakViolations.Count -eq 0)
}
$speakPlan | ConvertTo-Json -Depth 6
if ($speakViolations.Count -gt 0) { throw 'Build preflight refused. See violations above; no build was started and nothing was deleted.' }
if ($ValidateOnly) { return }

$speakOldTarget = [Environment]::GetEnvironmentVariable('CARGO_TARGET_DIR', 'Process')
$speakOldBuildDirectory = [Environment]::GetEnvironmentVariable('CARGO_BUILD_BUILD_DIR', 'Process')
$speakOldIncremental = [Environment]::GetEnvironmentVariable('CARGO_INCREMENTAL', 'Process')
$speakNativePreference = Get-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue
$speakHadNativePreference = ($null -ne $speakNativePreference)
$speakOldNativePreference = $(if ($speakHadNativePreference) { $speakNativePreference.Value } else { $null })
try {
    # Inspect native exit codes ourselves so PS7's opt-in native exceptions do
    # not skip the post-build cache report when Cargo fails.
    $PSNativeCommandUseErrorActionPreference = $false
    $env:CARGO_TARGET_DIR = $speakTarget
    # Newer Cargo can place intermediate artifacts in a separate build-dir.
    # Keep that directory in the same measured cache as the final artifacts.
    $env:CARGO_BUILD_BUILD_DIR = $speakTarget
    $env:CARGO_INCREMENTAL = '0'
    Push-Location -LiteralPath $speakApp
    try {
        & $speakTool.Source @speakArguments
        $speakBuildExit = $LASTEXITCODE
    } finally {
        Pop-Location
    }
    $speakAfter = Get-SpeakCacheMeasure $speakTarget
    $speakAfterDisks = @(Get-SpeakDiskMeasures)
    $speakAfterViolations = @(Get-SpeakBudgetViolations $speakAfter $speakAfterDisks)
    [ordered]@{
        task = $Task
        completed = ($speakBuildExit -eq 0)
        exitCode = $speakBuildExit
        targetDirectory = $speakTarget
        cache = $speakAfter
        cacheGrowthBytes = $speakAfter.bytes - $speakCache.bytes
        disks = $speakAfterDisks
        violations = $speakAfterViolations
    } | ConvertTo-Json -Depth 6
    if ($speakBuildExit -ne 0) { throw "$Task failed with exit code $speakBuildExit. Build artifacts were retained for diagnosis." }
    if ($speakAfterViolations.Count -gt 0) {
        throw 'Build completed, but the post-build budget check failed. Artifacts were retained; review them before the next build.'
    }
} finally {
    [Environment]::SetEnvironmentVariable('CARGO_TARGET_DIR', $speakOldTarget, 'Process')
    [Environment]::SetEnvironmentVariable('CARGO_BUILD_BUILD_DIR', $speakOldBuildDirectory, 'Process')
    [Environment]::SetEnvironmentVariable('CARGO_INCREMENTAL', $speakOldIncremental, 'Process')
    if ($speakHadNativePreference) {
        $PSNativeCommandUseErrorActionPreference = $speakOldNativePreference
    } else {
        Remove-Variable -Name PSNativeCommandUseErrorActionPreference -ErrorAction SilentlyContinue
    }
}
