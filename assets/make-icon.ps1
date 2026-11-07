# Regenerates assets/sigma5.ico (multi-size, PNG-compressed) and assets/sigma5.png
# from the SIGMA5 logo (SIGMA5_LOGO.svg in the repo root).
#
# The SVG is just a ring and three bars, so the shapes are reproduced here in
# the SVG's own coordinate space (viewBox 0 0 1325 1653, transforms applied)
# and rasterised with anti-aliasing. If the logo changes, update SHAPES below.
#
# Run: powershell -ExecutionPolicy Bypass -File assets/make-icon.ps1
Add-Type -AssemblyName System.Drawing

$VIEW_W = 1325.0
$VIEW_H = 1653.0
$COLOR = [System.Drawing.Color]::FromArgb(255, 171, 114, 255)

# Ring: centre, outer radius, inner radius.
$RINGDEF = @{ cx = 550.67; cy = 1101.34; ro = 550.67; ri = 275.33 }
# Bars: x, y, width, height.
$BARS = @(
    @(0.0,    0.0,    1101.3, 275.34),   # top
    @(0.0,    0.0,    275.33, 1101.3),   # left
    @(550.67, 550.67, 774.23, 275.33)    # middle
)

function Draw-Logo([int]$size, [double]$margin) {
    $bmp = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = 'AntiAlias'
    $g.PixelOffsetMode = 'HighQuality'
    $g.Clear([System.Drawing.Color]::Transparent)

    $avail = $size * (1.0 - 2.0 * $margin)
    $k = [Math]::Min($avail / $VIEW_W, $avail / $VIEW_H)
    $ox = ($size - $VIEW_W * $k) / 2.0
    $oy = ($size - $VIEW_H * $k) / 2.0
    $brush = New-Object System.Drawing.SolidBrush $COLOR

    # Ring (alternate fill makes the hole).
    $ringPath = New-Object System.Drawing.Drawing2D.GraphicsPath
    $ringPath.FillMode = 'Alternate'
    foreach ($r in @($RINGDEF.ro, $RINGDEF.ri)) {
        $ringPath.AddEllipse([single]($ox + ($RINGDEF.cx - $r) * $k), [single]($oy + ($RINGDEF.cy - $r) * $k), [single](2 * $r * $k), [single](2 * $r * $k))
    }
    $g.FillPath($brush, $ringPath)

    # Bars are filled separately so overlaps simply union.
    foreach ($b in $BARS) {
        $g.FillRectangle($brush, [single]($ox + $b[0] * $k), [single]($oy + $b[1] * $k), [single]($b[2] * $k), [single]($b[3] * $k))
    }
    $g.Dispose()
    return $bmp
}

$sizes = 16, 20, 24, 32, 40, 48, 64, 128, 256
$pngs = @()
foreach ($s in $sizes) {
    # Small icons get less padding so the strokes stay legible.
    $margin = if ($s -le 24) { 0.02 } else { 0.06 }
    $bmp = Draw-Logo $s $margin
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngs += , @($s, $ms.ToArray())
    $bmp.Dispose()
}

# Pack into ICO.
$out = New-Object System.IO.MemoryStream
$bw = New-Object System.IO.BinaryWriter $out
$bw.Write([uint16]0); $bw.Write([uint16]1); $bw.Write([uint16]$pngs.Count)
$offset = 6 + 16 * $pngs.Count
foreach ($p in $pngs) {
    $s = $p[0]; $data = $p[1]
    $dim = if ($s -ge 256) { 0 } else { $s }
    $bw.Write([byte]$dim); $bw.Write([byte]$dim); $bw.Write([byte]0); $bw.Write([byte]0)
    $bw.Write([uint16]1); $bw.Write([uint16]32)
    $bw.Write([uint32]$data.Length); $bw.Write([uint32]$offset)
    $offset += $data.Length
}
foreach ($p in $pngs) { $bw.Write([byte[]]$p[1]) }
$bw.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $PSScriptRoot 'sigma5.ico'), $out.ToArray())

# README image.
$big = Draw-Logo 256 0.06
$big.Save((Join-Path $PSScriptRoot 'sigma5.png'), [System.Drawing.Imaging.ImageFormat]::Png)
$big.Dispose()
Write-Output "Wrote sigma5.ico ($($pngs.Count) sizes) and sigma5.png"
