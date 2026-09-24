[CmdletBinding()]
param(
    [string]$SourceImage,
    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
if (-not $SourceImage) {
    $SourceImage = Join-Path $repoRoot 'assets\app-icon-liquid-glass-neutral.png'
}
if (-not $OutputDirectory) {
    $OutputDirectory = Join-Path $PSScriptRoot 'Assets'
}

$sourcePath = (Resolve-Path -LiteralPath $SourceImage).Path
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$outputPath = (Resolve-Path -LiteralPath $OutputDirectory).Path

Add-Type -AssemblyName System.Drawing
$source = [System.Drawing.Image]::FromFile($sourcePath)
try {
    if ($source.Width -ne $source.Height) {
        throw "Source image must be square; got $($source.Width)x$($source.Height): $sourcePath"
    }

    $sizes = [ordered]@{
        'Square44x44Logo.png' = 44
        'Square150x150Logo.png' = 150
        'StoreLogo.png' = 50
    }
    foreach ($assetName in $sizes.Keys) {
        $size = [int]$sizes[$assetName]
        $bitmap = [System.Drawing.Bitmap]::new(
            $size,
            $size,
            [System.Drawing.Imaging.PixelFormat]::Format32bppArgb
        )
        try {
            $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
            try {
                $graphics.Clear([System.Drawing.Color]::Transparent)
                $graphics.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
                $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::HighQuality
                $graphics.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
                $graphics.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
                $graphics.DrawImage($source, 0, 0, $size, $size)
            }
            finally {
                $graphics.Dispose()
            }
            $destination = Join-Path $outputPath $assetName
            $bitmap.Save($destination, [System.Drawing.Imaging.ImageFormat]::Png)
            Write-Output "Wrote $destination ($size x $size)"
        }
        finally {
            $bitmap.Dispose()
        }
    }
}
finally {
    $source.Dispose()
}
