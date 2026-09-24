# MSIX compatibility notes

Liquid Start remains an ordinary Win32 executable. MSIX only adds a
Windows.Desktop package manifest and Store distribution path; no runtime code,
internal identifiers, Steam behavior, or GitHub ZIP format was changed for
packaging.

## Current implementation and checks

| Feature | Existing implementation | MSIX consideration and manual check |
| --- | --- | --- |
| Global Win+Space shortcut | Registers WH_KEYBOARD_LL with SetWindowsHookExW from the full-trust process. | The manifest grants full trust, not a special global-hook capability. Confirm hook registration, shortcut capture, suppression behavior, and cleanup on Windows 10/11 with the Store package installed. |
| Notification-area icon | Uses Shell_NotifyIconW on its own message-pump thread. | Confirm the icon appears, its menu works, and quitting/relaunching restores it. Also check behavior after Explorer restarts. |
| Liquid Glass capture | Uses Windows Graphics Capture and Direct3D interop. | Confirm GraphicsCaptureSession::IsSupported, capture permission/system UX, frame updates, and cleanup in a packaged launch on each supported OS/device. |
| Installed apps and Start Menu | Enumerates shell:AppsFolder and per-user/all-users Start Menu shortcut folders. | Confirm enumeration and launching for unpackaged Win32, packaged, and Store apps. The package itself may also appear in the AppsFolder scan. |
| Steam apps | Reads Steam registry locations and library manifests on disk. | Confirm registry discovery and access to each Steam library, especially libraries on another drive or protected location. |
| SQLite persistence | Opens %LOCALAPPDATA%\Launchpad\cache.sqlite3; the fallback without LOCALAPPDATA is beside the executable. | Full-trust MSIX documentation describes AppData access as pass-through, while other packaged-desktop documentation describes per-package handling for some virtualized-app scenarios and OS versions. Do not assume the result: test existing-file reads/writes, first creation, package upgrades, uninstall, and interoperation with the GitHub/Steam executable on the minimum supported Windows version. No data migration or path change is included. |
| Single instance | Uses the stable Local\LaunchpadWindowsSingleInstance mutex. | The identity and executable name remain the same; test simultaneous packaged and unpackaged launches and retain the existing single-instance behavior. |

The app uses %LOCALAPPDATA% rather than package-local storage, which is
important for compatibility with the current Win32 builds. The [current MSIX virtualization overview](https://learn.microsoft.com/en-us/windows/msix/msix-containerization-overview) describes full-trust AppData writes as pass-through. The [packaged desktop behavior reference](https://learn.microsoft.com/en-us/windows/msix/desktop/desktop-to-uwp-behind-the-scenes) also documents per-package handling for virtualized-app scenarios. Treat this as a runtime compatibility check on supported Windows versions rather than relying on one behavior.
The app keeps ordinary desktop access, but packaged process behavior and Windows-version differences should be verified before relying on cross-channel cache sharing. The MSIX package is not considered validated for release until
the database compatibility checks above pass.

## User data compatibility

The cache path remains %LOCALAPPDATA%\Launchpad\cache.sqlite3, and
diagnostic logs remain under %LOCALAPPDATA%\Launchpad. The crate name,
LAUNCHPAD_* variables, mutex, database compatibility IDs, QA identifiers,
and Rust module names are not changed by this packaging work.

Do not delete or migrate the cache as part of packaging. Test with a copy of a
real pre-existing cache first. The Store MSIX, GitHub ZIP, and Steam build use
the same Rust executable and code; whether Windows routes a particular AppData
operation to a package-specific location must be confirmed on the actual
target OS versions.

## Store package limits

Only the MSIX manifest requests package identity and runFullTrust. The
executable is still built for x86_64-pc-windows-msvc and does not require a
packaged process or WindowsApps install path when used from Steam or a GitHub
ZIP. See ../packaging/windows/README.md for building and signing.
