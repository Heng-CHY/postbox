<#
.SYNOPSIS
    Put `postbox` on your PATH so one command works from any folder (Windows, no admin).

.DESCRIPTION
    Writes a launcher to <项目目录>\scripts\postbox.cmd and adds that folder to your user
    PATH. The launcher pins --root to your data directory, because postbox resolves data/
    from the folder you are standing in — without it, running the command from elsewhere
    would refuse rather than quietly open a second, empty instance.

    The launcher contains your machine's absolute paths, so it is git-ignored. It goes in
    scripts\ rather than next to postbox.exe on purpose: Windows prefers .exe over .cmd, so
    a launcher sharing a folder with the binary would never be picked up.

    From the repository root:   .\scripts\win-path.ps1
    Next to a downloaded exe:   .\win-path.ps1
    Undo:                       .\scripts\win-path.ps1 -Uninstall

    Open a new window afterwards — already-open shells keep the old PATH.
#>
[CmdletBinding()]
param(
    [string]$ExeDir,
    [switch]$Uninstall
)

$ErrorActionPreference = 'Stop'

function Get-UserPath {
    $v = [Environment]::GetEnvironmentVariable('Path', 'User')
    if ([string]::IsNullOrEmpty($v)) { @() } else { @($v -split ';' | Where-Object { $_ }) }
}

function Set-UserPath([string[]]$Parts) {
    # Through the .NET API, never setx: setx silently truncates PATH to 1024 characters.
    [Environment]::SetEnvironmentVariable('Path', ($Parts -join ';'), 'User')
}

# The folder the launcher lives in, derived from where this script sits.
$ShimDir = if ((Split-Path -Leaf $PSScriptRoot) -eq 'scripts') { $PSScriptRoot }
           else { Join-Path $PSScriptRoot 'scripts' }
$Shim = Join-Path $ShimDir 'postbox.cmd'

if ($Uninstall) {
    if (Test-Path -LiteralPath $Shim) {
        Remove-Item -LiteralPath $Shim -Force
        Write-Host "deleted  $Shim"
    } else {
        Write-Host "nothing to delete at $Shim"
    }
    $kept = @(Get-UserPath | Where-Object { $_ -ne $ShimDir })
    if ($kept.Count -ne (Get-UserPath).Count) {
        Set-UserPath $kept
        Write-Host "removed from user PATH: $ShimDir"
    } else {
        Write-Host "user PATH did not contain $ShimDir"
    }
    Write-Host 'done. open a new window for the change to take effect.'
    return
}

if (-not $ExeDir) { $ExeDir = (Get-Location).ProviderPath }

$exe = Join-Path $ExeDir 'postbox.exe'
if (-not (Test-Path -LiteralPath $exe)) {
    $nested = Join-Path $ExeDir 'target\release\postbox.exe'
    if (Test-Path -LiteralPath $nested) { $exe = $nested } else { $exe = $null }
}
if (-not $exe) {
    throw "postbox.exe not found in '$ExeDir' or in 'target\release' under it — cd into that folder, or pass -ExeDir"
}
$exe = (Resolve-Path -LiteralPath $exe).Path

# A source build puts the binary in target\release, two levels below the project folder
# that holds data/; a downloaded release puts both in the same folder.
$exeDir = Split-Path -Parent $exe
$isTargetBuild = (Split-Path -Leaf $exeDir) -eq 'release' -and
                 (Split-Path -Leaf (Split-Path -Parent $exeDir)) -eq 'target'
$ProjectDir = if ($isTargetBuild) { Split-Path -Parent (Split-Path -Parent $exeDir) } else { $exeDir }
$Root = Join-Path $ProjectDir 'data'

if (-not (Test-Path -LiteralPath $ShimDir)) {
    New-Item -ItemType Directory -Path $ShimDir | Out-Null
}
Set-Content -LiteralPath $Shim -Encoding Default -Value @(
    '@echo off',
    "`"$exe`" --root `"$Root`" %*"
)

$parts = @(Get-UserPath)
if ($parts -contains $ShimDir) {
    Write-Host "user PATH already contains $ShimDir"
} else {
    Set-UserPath ($parts + $ShimDir)
    Write-Host "added to user PATH: $ShimDir"
}

Write-Host ''
Write-Host "launcher : $Shim"
Write-Host "  binary : $exe"
Write-Host "  data   : $Root"
Write-Host ''
Write-Host 'now open a NEW PowerShell window and run:  postbox --version'
