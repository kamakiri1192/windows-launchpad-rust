# Creates/removes a test .lnk in the user Start Menu Programs dir for the
# live-refresh verification. Usage:
#   powershell -File make_test_lnk.ps1 create [name]
#   powershell -File make_test_lnk.ps1 touch  [name]   # bump mtime
#   powershell -File make_test_lnk.ps1 remove [name]
param(
    [Parameter(Mandatory=$true)][string]$Action,
    [string]$Name = "LaunchpadRefreshTest"
)

$ErrorActionPreference = "Stop"
$dir = Join-Path $env:APPDATA "Microsoft\Windows\Start Menu\Programs"
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir | Out-Null }
$path = Join-Path $dir ("$Name.lnk")

switch ($Action) {
    "create" {
        $shell = New-Object -ComObject WScript.Shell
        $lnk = $shell.CreateShortcut($path)
        $lnk.TargetPath = "$env:WINDIR\System32\notepad.exe"
        $lnk.IconLocation = "$env:WINDIR\System32\notepad.exe,0"
        $lnk.Save()
        Write-Output "created $path"
    }
    "touch" {
        # Rewrite with a different target so mtime + target both change.
        $shell = New-Object -ComObject WScript.Shell
        $lnk = $shell.CreateShortcut($path)
        $lnk.TargetPath = "$env:WINDIR\System32\calc.exe"
        $lnk.IconLocation = "$env:WINDIR\System32\calc.exe,0"
        $lnk.Save()
        Write-Output "touched $path"
    }
    "remove" {
        if (Test-Path $path) { Remove-Item $path; Write-Output "removed $path" }
        else { Write-Output "absent $path" }
    }
    default { Write-Error "unknown action: $Action"; exit 1 }
}
