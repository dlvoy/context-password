<#
.SYNOPSIS
Regenerates resources/app.ico and resources/tray_icon.rgba from development/logo.png.

.DESCRIPTION
Not run by `cargo build` — a one-time (or as-needed) asset step, documented in BUILDING.md.
The source PNG (1337x1176, already has a transparent background) is padded onto a square
transparent canvas, then resized down to each icon size with high-quality interpolation.

app.ico is a modern PNG-compressed multi-resolution ICO (16/32/48/256), which every Windows
version since Vista accepts — building it by hand avoids adding an image crate to the Rust
project just for a one-time conversion.

tray_icon.rgba is a raw, headerless 32x32 RGBA buffer for `tray_icon::Icon::from_rgba` at
runtime — also to avoid a runtime PNG-decoding dependency.

.EXAMPLE
powershell -ExecutionPolicy Bypass -File resources/generate-icons.ps1
#>

Add-Type -AssemblyName System.Drawing

$root = Split-Path -Parent $PSScriptRoot
$source = Join-Path $root "development\logo.png"
$icoOut = Join-Path $PSScriptRoot "app.ico"
$rgbaOut = Join-Path $PSScriptRoot "tray_icon.rgba"

if (-not (Test-Path $source)) {
    Write-Error "source logo not found: $source"
    exit 1
}

function New-SquareCanvas([System.Drawing.Image]$src) {
    $size = [Math]::Max($src.Width, $src.Height)
    $square = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($square)
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g.Clear([System.Drawing.Color]::Transparent)
    $x = [int](($size - $src.Width) / 2)
    $y = [int](($size - $src.Height) / 2)
    $g.DrawImage($src, $x, $y, $src.Width, $src.Height)
    $g.Dispose()
    return $square
}

function Resize-Square([System.Drawing.Image]$square, [int]$size) {
    $bmp = New-Object System.Drawing.Bitmap($size, $size, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    $g.DrawImage($square, 0, 0, $size, $size)
    $g.Dispose()
    return $bmp
}

$src = [System.Drawing.Bitmap]::FromFile($source)
$square = New-SquareCanvas $src
$src.Dispose()

# --- app.ico: PNG-compressed multi-resolution ICO ---
$sizes = @(16, 32, 48, 256)
$pngBlobs = @()
foreach ($size in $sizes) {
    $resized = Resize-Square $square $size
    $ms = New-Object System.IO.MemoryStream
    $resized.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $pngBlobs += , $ms.ToArray()
    $ms.Dispose()
    $resized.Dispose()
}

$headerSize = 6
$entrySize = 16
$offset = $headerSize + $entrySize * $sizes.Count

$stream = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter($stream)

# ICONDIR: reserved(2)=0, type(2)=1 (icon), count(2)
$writer.Write([uint16]0)
$writer.Write([uint16]1)
$writer.Write([uint16]$sizes.Count)

for ($i = 0; $i -lt $sizes.Count; $i++) {
    $size = $sizes[$i]
    $blob = $pngBlobs[$i]
    # ICONDIRENTRY: width/height (0 means 256), colorCount, reserved, planes,
    # bitCount, bytesInRes, imageOffset
    $wh = if ($size -eq 256) { 0 } else { $size }
    $writer.Write([byte]$wh)
    $writer.Write([byte]$wh)
    $writer.Write([byte]0)      # color count (0 = no palette)
    $writer.Write([byte]0)      # reserved
    $writer.Write([uint16]1)    # color planes
    $writer.Write([uint16]32)   # bits per pixel
    $writer.Write([uint32]$blob.Length)
    $writer.Write([uint32]$offset)
    $offset += $blob.Length
}

foreach ($blob in $pngBlobs) {
    $writer.Write($blob)
}

$writer.Flush()
[System.IO.File]::WriteAllBytes($icoOut, $stream.ToArray())
$writer.Dispose()
$stream.Dispose()
Write-Host "wrote $icoOut ($($sizes -join ', ') px)"

# --- tray_icon.rgba: raw 32x32 RGBA, row-major, top-to-bottom ---
$traySize = 32
$trayBmp = Resize-Square $square $traySize
$rgba = New-Object byte[] ($traySize * $traySize * 4)
$idx = 0
for ($y = 0; $y -lt $traySize; $y++) {
    for ($x = 0; $x -lt $traySize; $x++) {
        $p = $trayBmp.GetPixel($x, $y)
        $rgba[$idx]     = $p.R
        $rgba[$idx + 1] = $p.G
        $rgba[$idx + 2] = $p.B
        $rgba[$idx + 3] = $p.A
        $idx += 4
    }
}
[System.IO.File]::WriteAllBytes($rgbaOut, $rgba)
$trayBmp.Dispose()
Write-Host "wrote $rgbaOut (${traySize}x${traySize} raw RGBA)"

$square.Dispose()
