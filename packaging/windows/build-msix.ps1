[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$PackageIdentityName,

    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$Publisher,

    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$PublisherDisplayName,

    [switch]$Sign,

    [string]$CertificatePath,

    [System.Security.SecureString]$CertificatePassword,

    [string]$OutputDirectory
)

$ErrorActionPreference = 'Stop'
$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$target = 'x86_64-pc-windows-msvc'
$binaryName = 'launchpad-windows.exe'
$templatePath = Join-Path $PSScriptRoot 'AppxManifest.xml'
$assetGenerator = Join-Path $PSScriptRoot 'generate-assets.ps1'

function Convert-CargoVersionToMsix([string]$cargoVersion) {
    if ($cargoVersion -notmatch '^(?<major>0|[1-9][0-9]*)\.(?<minor>0|[1-9][0-9]*)\.(?<patch>0|[1-9][0-9]*)$') {
        throw "Cargo version '$cargoVersion' must be a stable Major.Minor.Patch version without prerelease or build metadata for Store packaging."
    }

    $major = [System.Numerics.BigInteger]::Parse($Matches.major)
    $minor = [System.Numerics.BigInteger]::Parse($Matches.minor)
    $build = [System.Numerics.BigInteger]::Parse($Matches.patch)
    $max = [System.Numerics.BigInteger]::Parse('65535')

    if ($major -lt 1) {
        throw "Cargo version '$cargoVersion' is invalid for a Windows Store package: Major must be at least 1; 0.x.x is not remapped."
    }
    if ($major -gt $max) {
        throw "Cargo version '$cargoVersion' is invalid for a Windows Store package: Major must be at most 65535."
    }
    if ($minor -gt $max) {
        throw "Cargo version '$cargoVersion' is invalid for a Windows Store package: Minor must be at most 65535."
    }
    if ($build -gt $max) {
        throw "Cargo version '$cargoVersion' is invalid for a Windows Store package: Build (Cargo Patch) must be at most 65535."
    }

    return '{0}.{1}.{2}.0' -f $major, $minor, $build
}

function Get-MsixVersionFromCargo {
    $metadataJson = & cargo metadata --manifest-path (Join-Path $repoRoot 'Cargo.toml') --no-deps --format-version 1 --locked
    if ($LASTEXITCODE -ne 0) {
        throw "cargo metadata failed with exit code $LASTEXITCODE."
    }

    $metadata = ($metadataJson -join [Environment]::NewLine) | ConvertFrom-Json
    $packages = @($metadata.packages | Where-Object { $_.name -eq 'launchpad-windows' })
    if ($packages.Count -ne 1) {
        throw "Expected exactly one launchpad-windows package in Cargo metadata; found $($packages.Count)."
    }

    return Convert-CargoVersionToMsix ([string]$packages[0].version)
}

function Find-WindowsSdkTool([string]$ToolName) {
    $programFilesX86 = [System.Environment]::GetFolderPath([System.Environment+SpecialFolder]::ProgramFilesX86)
    $kitsRoot = Join-Path $programFilesX86 'Windows Kits\10\bin'
    if (-not (Test-Path -LiteralPath $kitsRoot)) {
        throw "Windows 10/11 SDK command-line tools were not found under '$kitsRoot'. Install the Windows SDK (MakeAppx.exe and SignTool.exe)."
    }

    $tool = Get-ChildItem -LiteralPath $kitsRoot -Directory |
        Where-Object { $_.Name -match '^\d+(\.\d+){3}$' } |
        Sort-Object { [version]$_.Name } -Descending |
        ForEach-Object {
            $candidate = Join-Path $_.FullName "x64\$ToolName"
            if (Test-Path -LiteralPath $candidate) {
                Get-Item -LiteralPath $candidate
            }
        } |
        Select-Object -First 1
    if (-not $tool) {
        throw "$ToolName was not found in '$kitsRoot'. Install the Windows SDK component that includes MSIX packaging tools."
    }
    return $tool.FullName
}

if ([string]::IsNullOrWhiteSpace($PackageIdentityName) -or
    [string]::IsNullOrWhiteSpace($Publisher) -or
    [string]::IsNullOrWhiteSpace($PublisherDisplayName)) {
    throw 'PackageIdentityName, Publisher, and PublisherDisplayName must contain the exact values from Partner Center.'
}
if ($PackageIdentityName -like '__*__' -or $Publisher -like '__*__' -or $PublisherDisplayName -like '__*__') {
    throw 'Replace the manifest placeholders with the exact Partner Center identity values before building.'
}
if ($Sign -and -not $CertificatePath) {
    throw 'CertificatePath is required when -Sign is specified.'
}
if (-not $Sign -and ($CertificatePath -or $CertificatePassword)) {
    throw 'CertificatePath and CertificatePassword are only accepted together with -Sign.'
}
if ($Sign -and -not (Test-Path -LiteralPath $CertificatePath -PathType Leaf)) {
    throw "Signing certificate was not found: $CertificatePath"
}
if ($Sign) {
    $CertificatePath = (Resolve-Path -LiteralPath $CertificatePath).Path
}

$msixVersion = Get-MsixVersionFromCargo
Write-Output "Cargo version maps to MSIX version $msixVersion."

Push-Location $repoRoot
$staging = Join-Path ([System.IO.Path]::GetTempPath()) "liquid-start-msix-$([guid]::NewGuid().ToString('N'))"
try {
    Write-Output "Building $binaryName for $target..."
    & cargo build --release --locked --target $target
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed with exit code $LASTEXITCODE."
    }

    $binaryPath = Join-Path $repoRoot "target\$target\release\$binaryName"
    if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
        throw "Expected release executable was not produced: $binaryPath"
    }

    New-Item -ItemType Directory -Force -Path $staging | Out-Null
    Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $staging $binaryName)

    $manifest = [xml](Get-Content -LiteralPath $templatePath -Raw)
    $namespaceManager = [System.Xml.XmlNamespaceManager]::new($manifest.NameTable)
    $namespaceManager.AddNamespace('appx', 'http://schemas.microsoft.com/appx/manifest/foundation/windows10')
    $identity = $manifest.SelectSingleNode('/appx:Package/appx:Identity', $namespaceManager)
    $identity.SetAttribute('Name', $PackageIdentityName)
    $identity.SetAttribute('Publisher', $Publisher)
    $identity.SetAttribute('Version', $msixVersion)
    $publisherDisplay = $manifest.SelectSingleNode('/appx:Package/appx:Properties/appx:PublisherDisplayName', $namespaceManager)
    $publisherDisplay.InnerText = $PublisherDisplayName
    $generatedManifest = Join-Path $staging 'AppxManifest.xml'
    $manifest.Save($generatedManifest)

    $stagedAssets = Join-Path $staging 'Assets'
    & $assetGenerator -OutputDirectory $stagedAssets

    $makeAppx = Find-WindowsSdkTool 'MakeAppx.exe'
    $signTool = if ($Sign) { Find-WindowsSdkTool 'SignTool.exe' } else { $null }
    if (-not $OutputDirectory) {
        $OutputDirectory = Join-Path $repoRoot 'dist'
    }
    elseif (-not [System.IO.Path]::IsPathRooted($OutputDirectory)) {
        $OutputDirectory = Join-Path $repoRoot $OutputDirectory
    }
    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    $outputDirectoryPath = (Resolve-Path -LiteralPath $OutputDirectory).Path
    $packageName = 'LiquidStart_{0}_x64.msix' -f $msixVersion
    $packagePath = Join-Path $outputDirectoryPath $packageName

    Write-Output "Packing with $makeAppx..."
    & $makeAppx pack /v /h SHA256 /d $staging /p $packagePath /o
    if ($LASTEXITCODE -ne 0) {
        throw "MakeAppx.exe failed with exit code $LASTEXITCODE."
    }

    if ($Sign) {
        $signArguments = @('sign', '/fd', 'SHA256', '/f', $CertificatePath)
        $passwordPointer = [IntPtr]::Zero
        try {
            if ($CertificatePassword) {
                $passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($CertificatePassword)
                $plainPassword = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer)
                $signArguments += @('/p', $plainPassword)
            }
            $signArguments += $packagePath
            & $signTool @signArguments
            if ($LASTEXITCODE -ne 0) {
                throw "SignTool.exe failed with exit code $LASTEXITCODE."
            }
        }
        finally {
            if ($passwordPointer -ne [IntPtr]::Zero) {
                [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer)
            }
            Remove-Variable plainPassword -ErrorAction SilentlyContinue
        }

        & $signTool verify /pa /v $packagePath
        if ($LASTEXITCODE -ne 0) {
            throw "SignTool.exe could not verify the signed package (exit code $LASTEXITCODE)."
        }
    }

    Write-Output "Created $packagePath"
}
finally {
    if (Test-Path -LiteralPath $staging) {
        $tempRoot = [System.IO.Path]::GetFullPath([System.IO.Path]::GetTempPath()).TrimEnd('\') + '\'
        $stagingPath = [System.IO.Path]::GetFullPath($staging)
        if (-not $stagingPath.StartsWith($tempRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
            throw "Refusing to remove staging directory outside the system temp root: $stagingPath"
        }
        Remove-Item -LiteralPath $stagingPath -Recurse -Force
    }
    Pop-Location
}
