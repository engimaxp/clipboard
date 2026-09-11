# 生成应用图标 icons/icon.png 和 icons/icon.ico（无外部依赖，仅用 System.Drawing）
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$outDir = Join-Path $PSScriptRoot '..\icons'
New-Item -ItemType Directory -Force $outDir | Out-Null

$size = 256
$bmp = New-Object System.Drawing.Bitmap($size, $size)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
$g.Clear([System.Drawing.Color]::FromArgb(30, 30, 46))

function New-RoundRect([int]$x, [int]$y, [int]$w, [int]$h, [int]$r) {
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $r * 2
    $p.AddArc($x, $y, $d, $d, 180, 90)
    $p.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
    $p.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
    $p.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
    $p.CloseFigure()
    return $p
}

# 剪贴板主体（蓝色渐变）
$body = New-RoundRect 54 74 148 150 22
$rect = New-Object System.Drawing.Rectangle(54, 74, 148, 150)
$grad = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
    $rect,
    [System.Drawing.Color]::FromArgb(137, 180, 250),
    [System.Drawing.Color]::FromArgb(114, 135, 253),
    90)
$g.FillPath($grad, $body)

# 文字行
$line1 = New-RoundRect 82 122 92 14 7
$brush1 = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(210, 255, 255, 255))
$g.FillPath($brush1, $line1)

$line2 = New-RoundRect 82 152 62 14 7
$brush2 = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(150, 255, 255, 255))
$g.FillPath($brush2, $line2)

# 顶部夹子
$clip = New-RoundRect 92 46 72 46 14
$clipBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(24, 24, 37))
$g.FillPath($clipBrush, $clip)

$clipTop = New-RoundRect 96 40 64 20 10
$clipTopBrush = New-Object System.Drawing.SolidBrush([System.Drawing.Color]::FromArgb(166, 227, 161))
$g.FillPath($clipTopBrush, $clipTop)

$g.Dispose()
$pngPath = Join-Path $outDir 'icon.png'
$bmp.Save($pngPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()

# 生成 ICO（内嵌 PNG 格式，Windows Vista+ 支持）
$pngBytes = [System.IO.File]::ReadAllBytes($pngPath)
$icoPath = Join-Path $outDir 'icon.ico'
$fs = [System.IO.File]::Create($icoPath)
$bw = New-Object System.IO.BinaryWriter($fs)
$bw.Write([uint16]0)              # reserved
$bw.Write([uint16]1)              # type: icon
$bw.Write([uint16]1)              # count
$bw.Write([byte]0)                # width  (0 = 256)
$bw.Write([byte]0)                # height (0 = 256)
$bw.Write([byte]0)                # palette
$bw.Write([byte]0)                # reserved
$bw.Write([uint16]1)              # planes
$bw.Write([uint16]32)             # bpp
$bw.Write([uint32]$pngBytes.Length)
$bw.Write([uint32]22)             # offset
$bw.Write($pngBytes)
$bw.Close()
$fs.Close()

Write-Host "icons generated: $pngPath, $icoPath"
