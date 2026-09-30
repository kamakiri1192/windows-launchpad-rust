# Liquid Start MSIX packaging

This directory produces a separate Windows MSIX package from the existing
launchpad-windows.exe. The Steam and GitHub ZIP builds remain ordinary Win32
distributions and do not use MSIX.

## Prerequisites

- Windows 10 or 11, with Windows PowerShell 5.1 or PowerShell 7.
- Rust 1.89 with the x86_64-pc-windows-msvc target installed.
- The Microsoft C++ Build Tools MSVC linker (link.exe) for Rust's MSVC
  target. The Visual Studio IDE is not required.
- A Windows 10/11 SDK installation containing MakeAppx.exe. SignTool.exe
  is also needed when creating a locally installable, signed package.

The packaging script finds the newest x64 SDK tools below
%ProgramFiles(x86)%\Windows Kits\10\bin. It does not call Visual Studio or
MSBuild.

## Partner Center identity

Do not invent these values. In Partner Center, open the app's Product
management > App identity page and copy the exact, case-sensitive values:

- Package/Identity/Name → PackageIdentityName
- Package/Identity/Publisher → Publisher
- Package/Properties/PublisherDisplayName → PublisherDisplayName

The first two form the package identity. The publisher must agree with the
signing certificate for signed sideloading packages. The script requires all
three values and writes them into a generated manifest in the staging folder;
the template retains the __PACKAGE_IDENTITY_NAME__, __PUBLISHER__, and
__PUBLISHER_DISPLAY_NAME__ markers.

## Build a Store package

From the repository root, pass the identity values copied from Partner Center:

    ./packaging/windows/build-msix.ps1 -PackageIdentityName 'value-from-partner-center' -Publisher 'CN=value-from-partner-center' -PublisherDisplayName 'value-from-partner-center'

The script reads the crate version from cargo metadata, then runs:

    cargo build --release --locked --target x86_64-pc-windows-msvc

It stages the executable and generated assets, fills AppxManifest.xml, and
calls the Windows SDK MakeAppx.exe. The unsigned output is:

    dist/LiquidStart_1.0.0.0_x64.msix

Store submissions do not require you to provide a signing certificate; the
Store replaces the package signature during submission. This script emits a
single-architecture .msix. Partner Center also accepts individual MSIX
packages, though Microsoft recommends an .msixupload when symbol files or
multiple packages need to be included.

## Version mapping

Cargo's stable Major.Minor.Patch becomes MSIX
Major.Minor.Build.Revision as Major.Minor.Patch.0. Thus 1.0.0 becomes
1.0.0.0 and 1.2.3 becomes 1.2.3.0.

The script rejects prerelease/build-metadata versions, Major 0, Major above
65535, and Minor or Patch above 65535. It never changes 0.x.x into 1.x.x.
MSIX revision is fixed at 0 for Store packages.

## Assets

generate-assets.ps1 deterministically resizes
assets/app-icon-liquid-glass-neutral.png into the three PNGs referenced by
the manifest:

- Square44x44Logo.png (44 x 44)
- Square150x150Logo.png (150 x 150)
- StoreLogo.png (50 x 50)

Run it from any directory to refresh the checked-in generated copies:

    ./packaging/windows/generate-assets.ps1

The package build regenerates the same files into its private staging folder.

## Local installation and test signing

An unsigned MSIX is intended for Store submission and cannot be installed as
a regular sideload package by default. For local install testing, create a test certificate
whose subject matches the Publisher value used in the manifest, export it
to a local PFX, and trust its public certificate on the test machine. Keep the
PFX and password outside source control.

For example, in an elevated PowerShell window, create and trust a local-only
test certificate (choose a subject reserved for local testing, not the Store
publisher):

    $testCert = New-SelfSignedCertificate -Type Custom -Subject 'CN=Liquid Start Local Test' -KeyUsage DigitalSignature -CertStoreLocation 'Cert:\CurrentUser\My' -TextExtension @('2.5.29.37={text}1.3.6.1.5.5.7.3.3')
    $password = Read-Host 'PFX password' -AsSecureString
    Export-PfxCertificate -Cert $testCert -FilePath .\liquid-start-test.pfx -Password $password
    Export-Certificate -Cert $testCert -FilePath .\liquid-start-test.cer
    Import-Certificate -FilePath .\liquid-start-test.cer -CertStoreLocation 'Cert:\CurrentUser\TrustedPeople'

Build with the matching Publisher and the test certificate:

    $password = Read-Host 'PFX password' -AsSecureString
    ./packaging/windows/build-msix.ps1 -PackageIdentityName 'a-local-test-package-name' -Publisher 'CN=Liquid Start Local Test' -PublisherDisplayName 'Liquid Start Local Test' -Sign -CertificatePath .\liquid-start-test.pfx -CertificatePassword $password

Install and remove the package from an ordinary PowerShell session:

    Add-AppxPackage -Path .\dist\LiquidStart_1.0.0.0_x64.msix
    Get-AppxPackage -Name 'a-local-test-package-name' | Remove-AppxPackage

This local test identity and certificate are not the values to submit to
Partner Center.

## Store review checklist

Before submission, verify the exact Partner Center identity and package
version, run the Windows App Certification Kit, and test install, launch,
upgrade, uninstall, and relaunch on supported Windows 10/11 devices. Review
../../docs/MSIX_COMPATIBILITY.md for the runtime compatibility checks that
need real packaged-app testing, including the existing SQLite cache.
See Microsoft Learn for [MakeAppx](https://learn.microsoft.com/en-us/windows/msix/package/create-app-package-with-makeappx-tool), [SignTool](https://learn.microsoft.com/en-us/windows/msix/package/sign-app-package-using-signtool), and the [Windows App Certification Kit](https://learn.microsoft.com/en-us/windows/uwp/debug-test-perf/windows-app-certification-kit).

The manifest uses the Windows.Desktop family, declares
Windows.FullTrustApplication, and requests runFullTrust. It does not make the
Rust executable depend on package identity, WindowsApps paths, or Store APIs.
