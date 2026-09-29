<#
.SYNOPSIS
    Put `postbox` on your PATH so one command works from any folder (Windows, no admin).

.DESCRIPTION
    Two user-level environment changes, nothing else:

      * POSTBOX_ROOT  gets the data directory (postbox reads it before falling back to
        ./data in the current folder, and an explicit --root still wins over it)
      * PATH          gets the folder holding postbox.exe

    Without POSTBOX_ROOT the command would only work while you sit in the instance folder,
    and would otherwise refuse to run rather than quietly start a second, empty instance.

    From the repository root:   .\scripts\win-path.ps1
    Next to a downloaded exe:   .\win-path.ps1
    Undo:                       .\scripts\win-path.ps1 -Uninstall

    Open a new window afterwards — and restart your editor if that is where you type the
    command, because its integrated terminal inherits the editor's own environment.
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

# The .NET API, never setx: setx silently truncates PATH to 1024 characters.
function Set-UserPath([string[]]$Parts) {
    [Environment]::SetEnvironmentVariable('Path', ($Parts -join ';'), 'User')
}

function Find-Exe([string]$Dir) {
    foreach ($c in (Join-Path $Dir 'postbox.exe'), (Join-Path $Dir 'target\release\postbox.exe')) {
        if (Test-Path -LiteralPath $c) { return (Resolve-Path -LiteralPath $c).Path }
    }
    $null
}

if (-not $ExeDir) { $ExeDir = (Get-Location).ProviderPath }
$exe = Find-Exe $ExeDir

if ($Uninstall) {
    [Environment]::SetEnvironmentVariable('POSTBOX_ROOT', $null, 'User')
    Write-Host 'cleared POSTBOX_ROOT'
    if ($exe) {
        $exeDir = Split-Path -Parent $exe
        $parts = @(Get-UserPath)
        $kept = @($parts | Where-Object { $_ -ne $exeDir })
        if ($kept.Count -ne $parts.Count) {
            Set-UserPath $kept
            Write-Host "removed from user PATH: $exeDir"
        } else {
            Write-Host "user PATH did not contain $exeDir"
        }
    } else {
        Write-Host "no postbox.exe under $ExeDir — clear the PATH entry by hand if it is stale"
    }
    Write-Host 'done. open a new window for it to take effect.'
    return
}

if (-not $exe) {
    throw "postbox.exe not found in '$ExeDir' or in 'target\release' under it — cd into that folder, or pass -ExeDir"
}

$exeDir = Split-Path -Parent $exe
# A source build keeps the binary in target\release, with data/ one level above target\.
$isTargetBuild = (Split-Path -Leaf $exeDir) -eq 'release' -and
                 (Split-Path -Leaf (Split-Path -Parent $exeDir)) -eq 'target'
$ProjectDir = if ($isTargetBuild) { Split-Path -Parent (Split-Path -Parent $exeDir) } else { $exeDir }
$Root = Join-Path $ProjectDir 'data'

[Environment]::SetEnvironmentVariable('POSTBOX_ROOT', $Root, 'User')
Write-Host "POSTBOX_ROOT = $Root"

$parts = @(Get-UserPath)
if ($parts -contains $exeDir) {
    Write-Host "user PATH already contains $exeDir"
} else {
    Set-UserPath ($parts + $exeDir)
    Write-Host "added to user PATH: $exeDir"
}

Write-Host ''
Write-Host "binary   : $exe"
Write-Host ''
Write-Host 'now open a NEW window (and restart your editor) and run:  postbox --version'
