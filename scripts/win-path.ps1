<#
.SYNOPSIS
    Make `postbox` runnable from any directory on Windows, without admin rights.

.DESCRIPTION
    Writes a small launcher to %LOCALAPPDATA%\Programs\postbox\postbox.cmd and puts that
    folder on your user PATH. The launcher pins --root to the data directory of your
    instance, because postbox resolves data/ from the current working directory and
    refuses to silently start a second instance somewhere else.

    Run it from the folder that holds postbox.exe (or from the repository root, which
    finds target\release\postbox.exe for you), or pass -ExeDir.

.PARAMETER ExeDir
    Folder containing postbox.exe. Defaults to the current directory, then to
    target\release under the current directory.

.PARAMETER Uninstall
    Delete the launcher and remove its folder from your user PATH.

.EXAMPLE
    .\scripts\win-path.ps1
    .\scripts\win-path.ps1 -ExeDir D:\postbox
    .\scripts\win-path.ps1 -Uninstall

.NOTES
    Already-open shells keep the old PATH: open a new window afterwards.
#>
[CmdletBinding()]
param(
    [string]$ExeDir,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'

$ShimDir = Join-Path $env:LOCALAPPDATA 'Programs\postbox'
$Shim = Join-Path $ShimDir 'postbox.cmd'

function Add-ToUserPath([string]$Dir) {
    $cur = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ([string]::IsNullOrEmpty($cur)) { $cur = '' }
    if (($cur -split ';') -contains $Dir) {
        Write-Host "user PATH already contains $Dir"
        return
    }
    # Written through the .NET API on purpose: setx truncates PATH to 1024 characters.
    [Environment]::SetEnvironmentVariable('Path', ($cur.TrimEnd(';') + ';' + $Dir), 'User')
    Write-Host "added to user PATH: $Dir"
}

function Remove-FromUserPath([string]$Dir) {
    $cur = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ([string]::IsNullOrEmpty($cur)) { return }
    $kept = ($cur -split ';' | Where-Object { $_ -and ($_ -ne $Dir) }) -join ';'
    [Environment]::SetEnvironmentVariable('Path', $kept, 'User')
    Write-Host "removed from user PATH: $Dir"
}

if ($Uninstall) {
    if (Test-Path -LiteralPath $Shim) {
        Remove-Item -LiteralPath $Shim -Force
        Write-Host "deleted $Shim"
    } else {
        Write-Host "no launcher at $Shim"
    }
    Remove-FromUserPath $ShimDir
    Write-Host 'done — open a new window for the PATH change to take effect'
    return
}

if (-not $ExeDir) { $ExeDir = (Get-Location).ProviderPath }

$exe = Join-Path $ExeDir 'postbox.exe'
$nested = Join-Path $ExeDir 'target\release\postbox.exe'
if (-not (Test-Path -LiteralPath $exe)) { $exe = '' }
if (-not $exe -and (Test-Path -LiteralPath $nested)) { $exe = $nested }
if (-not $exe) {
    throw "postbox.exe not found in '$ExeDir' or its target\release — cd into that folder, or pass -ExeDir"
}
$exe = (Resolve-Path -LiteralPath $exe).Path

# data/ sits next to the binary for a downloaded release, and one level above target\
# for a build from source.
$exeDir = Split-Path -Parent $exe
if ((Split-Path -Leaf $exeDir) -eq 'release' -and (Split-Path -Leaf (Split-Path -Parent $exeDir)) -eq 'target') {
    $root = Join-Path (Split-Path -Parent (Split-Path -Parent $exeDir)) 'data'
} else {
    $root = Join-Path $exeDir 'data'
}

New-Item -ItemType Directory -Force $ShimDir | Out-Null
$lines = @(
    '@echo off',
    "`"$exe`" --root `"$root`" %*"
)
# Default (ANSI) rather than ASCII: a Chinese user profile path would not survive ASCII.
Set-Content -LiteralPath $Shim -Value $lines -Encoding Default

Write-Host "launcher : $Shim"
Write-Host "  binary : $exe"
Write-Host "  data   : $root"
Add-ToUserPath $ShimDir
Write-Host 'now open a new window and run: postbox --version'
