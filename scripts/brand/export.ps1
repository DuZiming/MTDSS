# scripts/brand/export.ps1 — 品牌资产导出(V3-2 Task 1)
# 渲染器 resvg(cargo install resvg);adaptive-fg 合成固定用 System.Drawing。
# 产物清单 = 计划 Task 1 Interfaces:
#   tile-deep-{1024,192,144,96,72,48}.png / adaptive-bg-1024.png / adaptive-fg-1024.png /
#   mono-1024.png / tray-{light,dark}-{on,dim}-32.png
$ErrorActionPreference = 'Stop'

$root = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $PSCommandPath))   # file -> brand -> scripts -> repo root
$assets = Join-Path $root 'brand/assets'
$out = Join-Path $root 'brand/exported'
New-Item -ItemType Directory -Force $out | Out-Null

if (-not (Get-Command resvg -ErrorAction SilentlyContinue)) {
    throw "resvg 未安装:先 `cargo install resvg`(计划 Task 1 Step 2 前置)"
}

function Render([string]$svg, [string]$png, [int]$size) {
    & resvg --width $size --height $size $svg $png
    if ($LASTEXITCODE -ne 0) { throw "resvg 渲染失败: $svg -> $png" }
}

# 1) 主图标瓷片全档(1024=Tauri/iOS 源;48..192=安卓 API<26 mipmap 回退)
foreach ($s in 1024, 192, 144, 96, 72, 48) {
    Render (Join-Path $assets 'tile-deep.svg') (Join-Path $out "tile-deep-$s.png") $s
}

# 2) adaptive 三层(fg = 加边 SVG 单次渲染:mark 居中 62%,四周留白=安全区;
#    原 System.Drawing 合成方案在该脚本上下文 Bitmap 构造异常,弃用——见 ledger Ruling)
Render (Join-Path $assets 'adaptive-bg.svg') (Join-Path $out 'adaptive-bg-1024.png') 1024
Render (Join-Path $assets 'adaptive-fg.svg') (Join-Path $out 'adaptive-fg-1024.png') 1024
Render (Join-Path $assets 'mono-white.svg') (Join-Path $out 'mono-1024.png') 1024

# 3) 托盘四态 32px:mono 白/深 各渲一版;dim 版=stroke-opacity 0.4(临时 SVG,不入库)
function DimSvg([string]$srcSvg, [string]$color, [string]$tmpSvg) {
    $t = Get-Content $srcSvg -Raw
    $t = $t.Replace("stroke=`"$color`"", "stroke=`"$color`" stroke-opacity=`"0.4`"")
    [System.IO.File]::WriteAllText($tmpSvg, $t, (New-Object System.Text.UTF8Encoding($false)))
}
Render (Join-Path $assets 'mono-white.svg') (Join-Path $out 'tray-light-on-32.png') 32
Render (Join-Path $assets 'mono-dark.svg') (Join-Path $out 'tray-dark-on-32.png') 32
$tmpW = Join-Path $env:TEMP 'brand-mono-white-dim.svg'
$tmpD = Join-Path $env:TEMP 'brand-mono-dark-dim.svg'
DimSvg (Join-Path $assets 'mono-white.svg') '#F2F4FF' $tmpW
DimSvg (Join-Path $assets 'mono-dark.svg') '#1C2340' $tmpD
Render $tmpW (Join-Path $out 'tray-light-dim-32.png') 32
Render $tmpD (Join-Path $out 'tray-dark-dim-32.png') 32

Write-Host "导出完成 -> $out"
Get-ChildItem $out | Select-Object Name, Length | Format-Table -AutoSize
