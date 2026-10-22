# Saves a picture of a running app window (by process name) to a PNG, for checking the real app's screens.
# Uses Windows' PrintWindow with "render full content" so WebView2 pages are captured too.
param([string]$Process = 'sweepwise', [string]$Out = (Join-Path $PSScriptRoot 'capture.png'), [int]$WaitSeconds = 20)
Add-Type -AssemblyName System.Drawing
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class WinCap {
    [StructLayout(LayoutKind.Sequential)] public struct RECT { public int L, T, R, B; }
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
'@
# Real pixel sizes on scaled displays (otherwise only the top-left part of the window is captured).
[void][WinCap]::SetProcessDPIAware()
$p = $null
for ($i = 0; $i -lt $WaitSeconds * 2 -and -not $p; $i++) { $p = Get-Process -Name $Process -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1; if (-not $p) { Start-Sleep -Milliseconds 500 } }
if (-not $p) { "no window for $Process"; exit 1 }
$r = New-Object WinCap+RECT
[void][WinCap]::GetWindowRect($p.MainWindowHandle, [ref]$r)
$w = $r.R - $r.L; $h = $r.B - $r.T
$bmp = New-Object System.Drawing.Bitmap $w, $h
$g = [System.Drawing.Graphics]::FromImage($bmp); $hdc = $g.GetHdc()
$ok = [WinCap]::PrintWindow($p.MainWindowHandle, $hdc, 2)
$g.ReleaseHdc($hdc); $g.Dispose()
$bmp.Save($Out, [System.Drawing.Imaging.ImageFormat]::Png); $bmp.Dispose()
"window '$($p.MainWindowTitle)' ${w}x${h}, captured: $ok -> $Out"
