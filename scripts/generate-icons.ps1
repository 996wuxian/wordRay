# 使用指定 PNG 生成 WordRay 应用、文件及 NSIS 安装器图标。
# 沿用 doTime 的 Tauri 多尺寸图标和 NSIS 横幅规格。
param([string]$Source)

$ErrorActionPreference = 'Stop'
$projectRoot = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$iconSourcePath = Join-Path $projectRoot 'icon-source.png'
$iconsPath = Join-Path $projectRoot 'src-tauri\icons'
$publicPath = Join-Path $projectRoot 'public'
$temporaryIconsPath = Join-Path $projectRoot ('.icon-build-' + [Guid]::NewGuid().ToString('N'))

Add-Type -AssemblyName System.Drawing
$sourceInputPath = if ($Source) { (Get-Item -LiteralPath $Source).FullName } else { $iconSourcePath }
$sourceImage = [Drawing.Image]::FromFile($sourceInputPath)
if ($sourceImage.Width -ne $sourceImage.Height) {
    $sourceImage.Dispose()
    throw '图标源图片必须为正方形。'
}

function Save-IconImage {
    param(
        [string]$Path, [int]$Width, [int]$Height, [int]$Size,
        [int]$Left = 0, [int]$Top = 0, [switch]$Installer
    )
    $pixelFormat = if ($Installer) { [Drawing.Imaging.PixelFormat]::Format24bppRgb }
        else { [Drawing.Imaging.PixelFormat]::Format32bppArgb }
    $bitmap = [Drawing.Bitmap]::new($Width, $Height, $pixelFormat)
    $graphics = [Drawing.Graphics]::FromImage($bitmap)
    try {
        $background = if ($Installer) { [Drawing.Color]::FromArgb(11, 13, 18) }
            else { [Drawing.Color]::Transparent }
        $graphics.Clear($background)
        $graphics.InterpolationMode = [Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
        $graphics.PixelOffsetMode = [Drawing.Drawing2D.PixelOffsetMode]::HighQuality
        $graphics.SmoothingMode = [Drawing.Drawing2D.SmoothingMode]::HighQuality
        $attributes = [Drawing.Imaging.ImageAttributes]::new()
        try {
            $attributes.SetWrapMode([Drawing.Drawing2D.WrapMode]::TileFlipXY)
            $rectangle = [Drawing.Rectangle]::new($Left, $Top, $Size, $Size)
            $graphics.DrawImage($sourceImage, $rectangle, 0, 0,
                $sourceImage.Width, $sourceImage.Height, [Drawing.GraphicsUnit]::Pixel, $attributes)
            $format = if ($Installer) { [Drawing.Imaging.ImageFormat]::Bmp }
                else { [Drawing.Imaging.ImageFormat]::Png }
            $bitmap.Save($Path, $format)
        } finally { $attributes.Dispose() }
    } finally {
        $graphics.Dispose()
        $bitmap.Dispose()
    }
}

try {
    if ($sourceInputPath -ne $iconSourcePath) {
        Copy-Item -LiteralPath $sourceInputPath -Destination $iconSourcePath -Force
    }
    New-Item -ItemType Directory -Path $iconsPath, $publicPath, $temporaryIconsPath -Force | Out-Null
    Push-Location $projectRoot
    try {
        & node 'node_modules/@tauri-apps/cli/tauri.js' icon $iconSourcePath --output $temporaryIconsPath
        if ($LASTEXITCODE -ne 0) { throw "Tauri 图标生成失败（退出码 $LASTEXITCODE）。" }
    } finally { Pop-Location }
    # 本项目发布 Windows 版；复制桌面图标，避免加入不使用的移动端资源。
    Get-ChildItem -LiteralPath $temporaryIconsPath -File | ForEach-Object {
        Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $iconsPath $_.Name) -Force
    }
    Save-IconImage -Path (Join-Path $iconsPath '64x64.png') -Width 64 -Height 64 -Size 64
    Save-IconImage -Path (Join-Path $publicPath 'logo.png') -Width 512 -Height 512 -Size 512
    Save-IconImage -Path (Join-Path $iconsPath 'nsis-header.bmp') -Width 150 -Height 57 -Size 45 -Left 53 -Top 6 -Installer
    Save-IconImage -Path (Join-Path $iconsPath 'nsis-sidebar.bmp') -Width 164 -Height 314 -Size 120 -Left 22 -Top 97 -Installer
    Write-Host 'WordRay 应用图标、网页/划词图标和 NSIS 安装器图标已生成。'
} finally {
    $sourceImage.Dispose()
    # 只删除本脚本在项目内创建的临时目录。
    $resolvedTemporaryPath = [IO.Path]::GetFullPath($temporaryIconsPath)
    if ($resolvedTemporaryPath.StartsWith($projectRoot + [IO.Path]::DirectorySeparatorChar) -and
        (Test-Path -LiteralPath $resolvedTemporaryPath)) {
        Remove-Item -LiteralPath $resolvedTemporaryPath -Recurse -Force
    }
}
