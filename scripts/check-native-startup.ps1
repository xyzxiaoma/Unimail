param(
    [string]$TargetRoot = "target/release"
)

$ErrorActionPreference = "Stop"

function Resolve-TargetRoot {
    param([string]$RequestedRoot)

    if ([System.IO.Path]::IsPathRooted($RequestedRoot)) {
        return [System.IO.Path]::GetFullPath($RequestedRoot)
    }

    $repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot ".."))
    return [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $RequestedRoot))
}

function Resolve-UnimailExecutable {
    param([string]$ReleaseRoot)

    if ($IsWindows) {
        $path = Join-Path $ReleaseRoot "unimail.exe"
        if (Test-Path -LiteralPath $path -PathType Leaf) {
            return (Resolve-Path -LiteralPath $path).Path
        }
    }

    if ($IsMacOS) {
        $bundleRoot = Join-Path $ReleaseRoot "bundle/macos"
        $candidate = Get-ChildItem -LiteralPath $bundleRoot -Recurse -File -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '[/\\]Contents[/\\]MacOS[/\\][^/\\]+$' } |
            Select-Object -First 1
        if ($candidate) {
            return $candidate.FullName
        }

        # Tauri may remove the intermediate .app after producing a DMG. The same
        # native executable remains under the selected target's release root.
        $releasePath = Join-Path $ReleaseRoot "unimail"
        if (Test-Path -LiteralPath $releasePath -PathType Leaf) {
            return (Resolve-Path -LiteralPath $releasePath).Path
        }
    }

    throw "没有找到当前平台的 Unimail 原生可执行文件，请先运行对应 target 的 Tauri build。"
}

$releaseRoot = Resolve-TargetRoot -RequestedRoot $TargetRoot
$executable = Resolve-UnimailExecutable -ReleaseRoot $releaseRoot
$temporaryRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$stdoutPath = Join-Path $temporaryRoot "unimail-native-startup.stdout.log"
$stderrPath = Join-Path $temporaryRoot "unimail-native-startup.stderr.log"
Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue

$process = $null
try {
    $process = Start-Process -FilePath $executable -WorkingDirectory (Split-Path $executable) `
        -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath -PassThru
    Start-Sleep -Seconds 5
    $process.Refresh()
    if ($process.HasExited) {
        $stdout = if (Test-Path -LiteralPath $stdoutPath) { Get-Content -Raw -LiteralPath $stdoutPath } else { "" }
        $stderr = if (Test-Path -LiteralPath $stderrPath) { Get-Content -Raw -LiteralPath $stderrPath } else { "" }
        throw "Unimail 原生进程启动后提前退出（exit=$($process.ExitCode)）。`nstdout:`n$stdout`nstderr:`n$stderr"
    }
    Write-Output "Unimail 原生启动冒烟检查通过。"
}
finally {
    if ($process) {
        $process.Refresh()
        if (-not $process.HasExited) {
            Stop-Process -Id $process.Id -Force
            Wait-Process -Id $process.Id -Timeout 10 -ErrorAction SilentlyContinue
        }
    }
    Remove-Item -LiteralPath $stdoutPath, $stderrPath -Force -ErrorAction SilentlyContinue
}
