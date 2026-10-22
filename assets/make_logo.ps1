# Draws the Sweepwise logo: the teal shield with a white broom sweeping inside it and two sparkles.
# Writes logo-1024.png (for `npm run icons`, which makes every size Windows, macOS and Linux need) plus
# small previews (64/32/16 px) to check the broom still reads at taskbar size.
# All shapes use a 0-100 grid so they match the SVG logo in src/index.html exactly.
Add-Type -AssemblyName System.Drawing
$out = $PSScriptRoot

function New-LogoBitmap([int]$S) {
    $bmp = New-Object System.Drawing.Bitmap $S, $S
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'; $g.PixelOffsetMode = 'HighQuality'; $g.Clear([System.Drawing.Color]::Transparent)
    $k = $S / 100.0
    function P([double]$x, [double]$y) { New-Object System.Drawing.PointF ([float]($x * $k)), ([float]($y * $k)) }
    function C([int]$a, [int]$r, [int]$gg, [int]$b) { [System.Drawing.Color]::FromArgb($a, $r, $gg, $b) }

    # Shield
    $shield = New-Object System.Drawing.Drawing2D.GraphicsPath
    $shield.AddBezier((P 50 5), (P 62 13), (P 76 16), (P 90 17))
    $shield.AddBezier((P 90 17), (P 92 52), (P 80 80), (P 50 96))
    $shield.AddBezier((P 50 96), (P 20 80), (P 8 52), (P 10 17))
    $shield.AddBezier((P 10 17), (P 24 16), (P 38 13), (P 50 5))
    $shield.CloseFigure()
    $grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush((P 20 5), (P 80 96), (C 255 94 234 212), (C 255 13 148 136))
    $g.FillPath($grad, $shield)

    # Everything inside stays inside the shield.
    $g.SetClip($shield)

    # Broom shapes (handle, band, bristles) on the 0-100 grid; drawn twice: soft shadow, then white.
    $bristles = New-Object System.Drawing.Drawing2D.GraphicsPath
    $bristles.AddLine((P 37.18 53.98), (P 49.27 62.86))
    $bristles.AddLine((P 49.27 62.86), (P 44.07 82.61))
    $bristles.AddBezier((P 44.07 82.61), (P 35.0 80.6), (P 25.0 73.0), (P 19.89 64.85))
    $bristles.CloseFigure()
    $band = New-Object System.Drawing.Drawing2D.GraphicsPath
    $band.AddPolygon([System.Drawing.PointF[]]@((P 39.95 51.05), (P 51.23 59.34), (P 48.87 62.56), (P 37.58 54.27)))
    foreach ($pass in 'shadow', 'main') {
        $dx = if ($pass -eq 'shadow') { 1.3 } else { 0 }; $dy = if ($pass -eq 'shadow') { 1.6 } else { 0 }
        $m = New-Object System.Drawing.Drawing2D.Matrix; $m.Translate([float]($dx * $k), [float]($dy * $k))
        $white = if ($pass -eq 'shadow') { New-Object System.Drawing.SolidBrush (C 70 4 40 40) } else { [System.Drawing.Brushes]::White }
        $hp = New-Object System.Drawing.Pen ($(if ($pass -eq 'shadow') { C 70 4 40 40 } else { C 255 255 255 255 })), ([float][Math]::Max(1.6, 4.6 * $k))
        $hp.StartCap = 'Round'; $hp.EndCap = 'Round'
        $g.DrawLine($hp, (P (70.5 + $dx) (20.5 + $dy)), (P (45.6 + $dx) (55.2 + $dy)))
        $b2 = $bristles.Clone(); $b2.Transform($m); $g.FillPath($white, $b2)
        if ($pass -eq 'main') {
            $bd = $band.Clone(); $g.FillPath((New-Object System.Drawing.SolidBrush (C 255 10 16 32)), $bd)
        }
    }
    # Bristle lines (only where they can be seen).
    if ($S -ge 48) {
        $lp = New-Object System.Drawing.Pen (C 130 15 118 110), ([float](1.1 * $k))
        foreach ($f in 0.22, 0.44, 0.66, 0.84) {
            $tx = 37.18 + (49.27 - 37.18) * $f; $ty = 53.98 + (62.86 - 53.98) * $f
            $bx = 19.89 + (44.07 - 19.89) * $f; $by = 64.85 + (82.61 - 64.85) * $f
            $g.DrawLine($lp, (P ($tx + ($bx - $tx) * 0.12) ($ty + ($by - $ty) * 0.12)), (P ($bx - ($bx - $tx) * 0.06) ($by - ($by - $ty) * 0.06)))
        }
    }
    # Sparkles: clean, and a little bit wise.
    function Star([double]$cx, [double]$cy, [double]$r, $brush) {
        $s = New-Object System.Drawing.Drawing2D.GraphicsPath
        $s.AddBezier((P $cx ($cy - $r)), (P $cx $cy), (P $cx $cy), (P ($cx + $r) $cy))
        $s.AddBezier((P ($cx + $r) $cy), (P $cx $cy), (P $cx $cy), (P $cx ($cy + $r)))
        $s.AddBezier((P $cx ($cy + $r)), (P $cx $cy), (P $cx $cy), (P ($cx - $r) $cy))
        $s.AddBezier((P ($cx - $r) $cy), (P $cx $cy), (P $cx $cy), (P $cx ($cy - $r)))
        $s.CloseFigure(); $g.FillPath($brush, $s)
    }
    Star 69 62 7 ([System.Drawing.Brushes]::White)
    if ($S -ge 32) { Star 78.5 48 4 (New-Object System.Drawing.SolidBrush (C 230 204 251 241)) }
    $g.ResetClip()
    if ($S -ge 32) { $g.DrawPath((New-Object System.Drawing.Pen (C 110 255 255 255), ([float][Math]::Max(1, 1.8 * $k))), $shield) }
    $g.Dispose()
    $bmp
}

$b = New-LogoBitmap 1024; $b.Save((Join-Path $out 'logo-1024.png'), [System.Drawing.Imaging.ImageFormat]::Png); $b.Dispose()
# Preview sheet: the small sizes side by side on the app's dark background, scaled up 4x so they can be judged.
$sheet = New-Object System.Drawing.Bitmap 520, 200
$gs = [System.Drawing.Graphics]::FromImage($sheet); $gs.Clear([System.Drawing.Color]::FromArgb(255, 10, 16, 32)); $gs.InterpolationMode = 'NearestNeighbor'
$x = 10
foreach ($sz in 64, 32, 16) {
    $sm = New-LogoBitmap $sz
    $gs.DrawImage($sm, $x, 10, $sz * ([Math]::Min(4, 180 / $sz)), $sz * ([Math]::Min(4, 180 / $sz)))
    $x += [int]($sz * [Math]::Min(4, 180 / $sz)) + 20; $sm.Dispose()
}
$gs.Dispose(); $sheet.Save((Join-Path $out 'logo-small-preview.png'), [System.Drawing.Imaging.ImageFormat]::Png); $sheet.Dispose()
"logo-1024.png: $((Get-Item (Join-Path $out 'logo-1024.png')).Length) bytes; preview sheet written"
