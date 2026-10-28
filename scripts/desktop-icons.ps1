# Desktop icon state, for the "PecoFence hid the icons and died" case.
#
#   powershell -File scripts/desktop-icons.ps1 -Status   # what Explorer thinks
#   powershell -File scripts/desktop-icons.ps1 -Show     # show them again (no-op when visible)
#
# The same two calls PecoFence makes: `SHELLSTATE.fHideIcons` for the state and
# `WM_COMMAND 0x7402` to the desktop's SHELLDLL_DefView for the toggle (the command behind
# Explorer's 查看 → 显示桌面图标). Works on Progman and on WorkerW hosts.
param(
    [switch]$Status,
    [switch]$Show
)

if (-not $Status -and -not $Show) {
    Write-Host 'usage: desktop-icons.ps1 -Status | -Show'
    exit 2
}

$source = @'
using System;
using System.Runtime.InteropServices;

public static class DeskIcons
{
    private delegate bool EnumProc(IntPtr hWnd, IntPtr lParam);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern IntPtr FindWindowEx(IntPtr parent, IntPtr after, string cls, string title);

    [DllImport("user32.dll")]
    private static extern bool EnumWindows(EnumProc cb, IntPtr lParam);

    [DllImport("user32.dll")]
    private static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint msg, IntPtr wParam,
        IntPtr lParam, uint flags, uint timeout, out IntPtr result);

    private static IntPtr _found;

    public static IntPtr FindDefView()
    {
        IntPtr progman = FindWindowEx(IntPtr.Zero, IntPtr.Zero, "Progman", null);
        if (progman != IntPtr.Zero)
        {
            IntPtr view = FindWindowEx(progman, IntPtr.Zero, "SHELLDLL_DefView", null);
            if (view != IntPtr.Zero) return view;
        }
        _found = IntPtr.Zero;
        EnumWindows((h, l) =>
        {
            IntPtr view = FindWindowEx(h, IntPtr.Zero, "SHELLDLL_DefView", null);
            if (view == IntPtr.Zero) return true;
            _found = view;
            return false;
        }, IntPtr.Zero);
        return _found;
    }

    public static bool Toggle()
    {
        IntPtr view = FindDefView();
        if (view == IntPtr.Zero) return false;
        IntPtr result;
        SendMessageTimeout(view, 0x0111, (IntPtr)0x7402, IntPtr.Zero, 0x0002, 2000, out result);
        return true;
    }

    public static string Version()
    {
        return "2";
    }
}
'@

$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Explorer\Advanced'
function Get-Hidden {
    (Get-ItemProperty -Path $key -Name HideIcons -ErrorAction SilentlyContinue).HideIcons
}

Add-Type -TypeDefinition $source
$view = [DeskIcons]::FindDefView()

if ($Status) {
    Write-Output ("HideIcons      : {0}" -f (Get-Hidden))
    Write-Output ("SHELLDLL_DefView: {0}" -f $(if ($view -eq [IntPtr]::Zero) { 'not found' } else { '0x{0:X}' -f $view.ToInt64() }))
    Write-Output ("pecofence procs : {0}" -f @(Get-Process -Name 'pecofence*' -ErrorAction SilentlyContinue).Count)
    # Test instances keep their own marker (icons-hidden.<instance>.marker).
    $markers = @(Get-ChildItem (Join-Path $env:LOCALAPPDATA 'PecoFence') -Filter 'icons-hidden*.marker' -ErrorAction SilentlyContinue)
    Write-Output ("markers         : {0}" -f $(if ($markers.Count -eq 0) { 'none' } else { ($markers | ForEach-Object { $_.Name }) -join ', ' }))
    exit 0
}

$hidden = Get-Hidden
if ($hidden -ne 1) {
    Write-Output 'icons are visible; nothing to do'
    exit 0
}
if ($view -eq [IntPtr]::Zero) {
    Write-Output 'SHELLDLL_DefView not found; restart Explorer to bring the icons back'
    exit 1
}
$sent = [DeskIcons]::Toggle()
Start-Sleep -Milliseconds 600
Write-Output ("toggle sent: {0}; HideIcons now: {1}" -f $sent, (Get-Hidden))
$marker = Join-Path $env:LOCALAPPDATA 'PecoFence\icons-hidden.marker'
if ((Get-Hidden) -ne 1 -and (Test-Path $marker)) {
    Remove-Item -LiteralPath $marker -Force
    Write-Output 'stale crash marker removed'
}
