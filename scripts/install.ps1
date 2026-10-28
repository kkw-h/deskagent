# Install (or uninstall) a local release build: %LOCALAPPDATA%\Programs\PecoFence,
# a Start-menu shortcut, and the login-startup entry.
#
#   powershell -File scripts/install.ps1                 # build dir auto-detected
#   powershell -File scripts/install.ps1 -Source <dir>   # a specific payload folder
#   powershell -File scripts/install.ps1 -NoAutostart    # do not start with Windows
#   powershell -File scripts/install.ps1 -Uninstall      # remove app, shortcut, startup entry
#
# The app reconciles the startup entry itself on every launch (it points the entry at the
# running copy), so this script only seeds it; -NoAutostart leaves it to the settings switch.
param(
    [string]$Source = '',
    [string]$Dir = '',
    [switch]$NoAutostart,
    [switch]$Uninstall,
    [switch]$Purge
)

$ErrorActionPreference = 'Stop'
$appName = 'PecoFence'
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$startMenu = Join-Path ([Environment]::GetFolderPath('Programs')) $appName
if (-not $Dir) { $Dir = Join-Path $env:LOCALAPPDATA "Programs\$appName" }
$exe = Join-Path $Dir 'pecofence.exe'

function Remove-StartupEntry {
    if ((Get-ItemProperty -Path $runKey -Name $appName -ErrorAction SilentlyContinue)) {
        Remove-ItemProperty -Path $runKey -Name $appName -Force
        Write-Host "startup entry removed"
    }
}

if ($Uninstall) {
    Remove-StartupEntry
    $link = Join-Path $startMenu "$appName.lnk"
    if (Test-Path $link) { Remove-Item $link -Force; Write-Host "shortcut removed" }
    if ((Test-Path $startMenu) -and -not (Get-ChildItem $startMenu -Force)) { Remove-Item $startMenu -Force }
    if (Test-Path $Dir) {
        # The watchdog is parented to Explorer and exits with its app; give it a moment.
        Get-Process -Name 'pecofence', 'pecofence-watchdog' -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -like "$Dir*" } |
            ForEach-Object { $_.Kill() }
        Start-Sleep -Milliseconds 500
        Remove-Item $Dir -Recurse -Force
        Write-Host "removed $Dir"
    }
    if ($Purge) {
        foreach ($p in @((Join-Path $env:APPDATA $appName), (Join-Path $env:LOCALAPPDATA $appName))) {
            if (Test-Path $p) { Remove-Item $p -Recurse -Force; Write-Host "removed $p" }
        }
    } else {
        Write-Host "kept %APPDATA%\PecoFence (configuration) and the desktop-icon state"
    }
    exit 0
}

if (-not $Source) {
    $cargo = (Get-Command cargo -ErrorAction SilentlyContinue).Source
    if (-not $cargo) { $cargo = Join-Path $env:USERPROFILE '.cargo\bin\cargo.exe' }
    if (-not (Test-Path $cargo)) { throw 'cargo not found; pass -Source <dir with the release exes>' }
    $target = (& $cargo metadata --no-deps --format-version 1 | ConvertFrom-Json).target_directory
    $Source = Join-Path $target 'x86_64-pc-windows-msvc\release'
    if (-not (Test-Path $Source)) { $Source = Join-Path $target 'release' }
}
$Source = (Resolve-Path $Source).Path
Write-Host "payload: $Source"

foreach ($name in 'pecofence.exe', 'pecofence-watchdog.exe') {
    if (-not (Test-Path (Join-Path $Source $name))) { throw "$name is missing in $Source (build with: cargo build --release -p pecofence -p pecofence-watchdog -p pecofence-cli)" }
}

New-Item -ItemType Directory -Force -Path $Dir | Out-Null
$files = 'pecofence.exe', 'pecofence-watchdog.exe', 'pecofence-cli.exe'
foreach ($name in $files) {
    $from = Join-Path $Source $name
    if (Test-Path $from) { Copy-Item $from (Join-Path $Dir $name) -Force }
}
# Licences and the agent guide live in the repository, not in the build output.
$fromRepo = @{
    'LICENSE' = 'LICENSE'
    'NOTICE' = 'NOTICE'
    'SKILL.md' = 'skills\pecofence-cli\SKILL.md'
    'UPGRADING.md' = 'docs\UPGRADING.md'
    'README.md' = 'README.md'
}
foreach ($dest in $fromRepo.Keys) {
    $from = Join-Path (Join-Path $PSScriptRoot '..') $fromRepo[$dest]
    if (Test-Path $from) { Copy-Item $from (Join-Path $Dir $dest) -Force }
}
# WebView2Loader.dll is imported at startup and must sit next to the exe. Cargo does not copy
# it (it comes from third_party); accept it in the payload or in the repository.
$loader = Join-Path $Source 'WebView2Loader.dll'
if (-not (Test-Path $loader)) { $loader = Join-Path $Source 'WebView2Loader.x64.dll' }
if (-not (Test-Path $loader)) { $loader = Join-Path $PSScriptRoot '..\third_party\webview2\WebView2Loader.x64.dll' }
if (-not (Test-Path $loader)) { throw 'WebView2Loader.x64.dll not found (third_party/webview2)' }
Copy-Item $loader (Join-Path $Dir 'WebView2Loader.dll') -Force
$webviewLicense = Join-Path $PSScriptRoot '..\third_party\webview2\LICENSE.txt'
if (Test-Path $webviewLicense) { Copy-Item $webviewLicense (Join-Path $Dir 'LICENSE-WebView2Loader.txt') -Force }
foreach ($name in 'SKILL.md', 'README.md', 'UPGRADING.md') {
    $from = Join-Path $PSScriptRoot "..\$name"
    if (Test-Path $from) { Copy-Item $from (Join-Path $Dir $name) -Force }
}
# This repository's own notes, next to the binary (not part of the upstream package).
$notes = Join-Path $PSScriptRoot '..\docs\工程备忘.md'
if (Test-Path $notes) { Copy-Item $notes (Join-Path $Dir 'ENGINEERING-NOTES.md') -Force }
Write-Host "installed to $Dir"

# Start-menu shortcut. Creating it needs the shell's COM object; a locked-down session may
# refuse, and the app itself works without one.
try {
    New-Item -ItemType Directory -Force -Path $startMenu | Out-Null
    $shell = New-Object -ComObject WScript.Shell
    $link = $shell.CreateShortcut((Join-Path $startMenu "$appName.lnk"))
    $link.TargetPath = $exe
    $link.WorkingDirectory = $Dir
    $link.IconLocation = "$exe,0"
    $link.Save()
    Write-Host "shortcut: $(Join-Path $startMenu "$appName.lnk")"
} catch {
    Write-Warning "Start-menu shortcut not created: $($_.Exception.Message)"
}

if (-not $NoAutostart) {
    New-Item -Path $runKey -Force | Out-Null
    Set-ItemProperty -Path $runKey -Name $appName -Value "`"$exe`""
    Write-Host "starts with Windows: $runKey\$appName"
} else {
    Remove-StartupEntry
    Write-Host 'starts with Windows: no (switch it on under 设置 → 常规)'
}

Write-Host ''
Write-Host "Run it: $exe"
