# SVG → 多尺寸 ICO(assets/cue.ico)。
# 无第三方依赖:Edge headless 按每个目标尺寸直接矢量重栅格化
# (比从 512 降采样锐利),System.Drawing 仅作兜底;ICO 容器手卷
# (PNG-in-ICO,Vista+ 全尺寸合法)。
# 用法:powershell -File scripts/icon.ps1 [-Svg path] [-Out path]
param(
    [string]$Svg = "C:\Users\exqin\Desktop\cue\assets\cue.svg",
    [string]$Out = "C:\Users\exqin\Desktop\cue\assets\cue.ico"
)
$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

$sizes = @(16, 24, 32, 48, 64, 128, 256)
$work = Join-Path $env:TEMP "cue-icon-gen"
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue | Out-Null
New-Item -ItemType Directory -Force $work | Out-Null

# --- 找 Edge(目标平台必有) ---
$edge = @(
    "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe",
    "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe"
) | Where-Object { Test-Path $_ } | Select-Object -First 1
if (-not $edge) { throw "msedge.exe not found" }

# --- 逐尺寸矢量栅格化(透明背景) ---
# --user-data-dir 必须隔离:Edge 正在运行时,默认 profile 的 headless 直接拒跑。
# 参数走数组 splatting:PS5.1 给原生 exe 传 --flag="值" 会保留内层引号,
# Chromium 会解析失败。
# 源 SVG 的 width/height="100%" 在 headless 视口下解析不可靠(实测 256px
# 只截到局部)——每个尺寸生成显式 width/height=N 的临时 SVG,与视口解耦。
function Rasterize([string]$edge, [string]$svgPath, [int]$s, [string]$png, [string]$profile, [string]$work) {
    $sizedSvg = Join-Path $work "sized-$s.svg"
    $content = [System.IO.File]::ReadAllText($svgPath, [System.Text.Encoding]::UTF8) `
        -replace 'width="100%"', "width=`"$s`"" `
        -replace 'height="100%"', "height=`"$s`""
    [System.IO.File]::WriteAllText($sizedSvg, $content, [System.Text.UTF8Encoding]::new($false))
    $uri = ([System.Uri]::new($sizedSvg)).AbsoluteUri
    & $edge @(
        "--headless=new", "--disable-gpu", "--hide-scrollbars",
        "--user-data-dir=$profile",
        "--default-background-color=00000000",
        "--window-size=$s,$s",
        "--screenshot=$png",
        $uri
    ) 2>$null | Out-Null
    # Edge 进程返回 ≠ 文件落盘:轮询等待(最多 5 s),再给小段写盘余量。
    $deadline = (Get-Date).AddSeconds(5)
    while (-not (Test-Path $png) -and (Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 100
    }
    Start-Sleep -Milliseconds 300
}
$pngs = @{}
foreach ($s in $sizes) {
    $png = Join-Path $work "$s.png"
    Rasterize $edge (Get-Item $Svg).FullName $s $png "$work\profile" $work
    if (-not (Test-Path $png)) { throw "edge screenshot failed for ${s}px" }
    $bmp = [System.Drawing.Image]::FromFile($png)
    if ($bmp.Width -ne $s -or $bmp.Height -ne $s) {
        # 窗口尺寸被 Chromium 钳制时的兜底:先渲 512 再高质量缩小
        $bmp.Dispose()
        $big = Join-Path $work "512.png"
        if (-not (Test-Path $big)) {
            Rasterize $edge (Get-Item $Svg).FullName 512 $big "$work\profile" $work
        }
        $src = [System.Drawing.Image]::FromFile($big)
        $bmp = New-Object System.Drawing.Bitmap $s, $s
        $g = [System.Drawing.Graphics]::FromImage($bmp)
        $g.InterpolationMode = "HighQualityBicubic"
        $g.PixelOffsetMode = "Half"
        $g.DrawImage($src, 0, 0, $s, $s)
        $g.Dispose(); $src.Dispose()
        $bmp.Save($png, [System.Drawing.Imaging.ImageFormat]::Png)
        Write-Host "  ${s}px: downscaled fallback"
    } else {
        $bmp.Dispose()
        Write-Host "  ${s}px: vector raster"
    }
    $pngs[$s] = $png
}

# --- 手卷 ICO:ICONDIR + ICONDIRENTRY[] + PNG payloads ---
$ms = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $ms
$w.Write([uint16]0)              # reserved
$w.Write([uint16]1)              # type: icon
$w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
foreach ($s in $sizes) {
    $bytes = [System.IO.File]::ReadAllBytes($pngs[$s])
    $w.Write([byte]($s -band 0xFF)) # 256 记 0
    $w.Write([byte]($s -band 0xFF))
    $w.Write([byte]0)               # 调色板色数
    $w.Write([byte]0)               # reserved
    $w.Write([uint16]1)             # planes
    $w.Write([uint16]32)            # bitcount
    $w.Write([uint32]$bytes.Length)
    $w.Write([uint32]$offset)
    $offset += $bytes.Length
}
foreach ($s in $sizes) { $w.Write([System.IO.File]::ReadAllBytes($pngs[$s])) }
$w.Flush()
New-Item -ItemType Directory -Force (Split-Path $Out) | Out-Null
[System.IO.File]::WriteAllBytes($Out, $ms.ToArray())
$w.Dispose(); $ms.Dispose()
Remove-Item -Recurse -Force $work

# 校验:文件头 + 条目数
$head = [System.IO.File]::ReadAllBytes($Out)[0..5]
if ($head[0] -ne 0 -or $head[1] -ne 0 -or $head[2] -ne 1) { throw "bad ICO header" }
Write-Host "OK: $Out ($($sizes -join '/')) px, $((Get-Item $Out).Length) bytes"
