# Windows application catalog

The catalog collects all Shell applications (Win32 and packaged), Start Menu
links, desktop links and Steam manifests. Desktop Known Folders are scanned at
the top level, including OneDrive redirection. Startup folders and document
links are excluded. Windows Search's document/web index is not the catalog.

## Identity and default launch

IShellLinkW is queried for IPropertyStore and System.AppUserModel.ID. VT_EMPTY
means absent; read failure is distinct. IDs retain their spelling and case.
AppsFolder provides Shell identity and locator. Labels never suppress apps.

- A shared nonempty AUMID means one app even when target, arguments, CWD,
  elevation or display settings differ. Different explicit IDs stay separate.
- Unidentified no-argument links to the same existing native exe can join one app.
  Registered IsHostApp executables are excluded. An unidentified link cannot
  bridge different explicit IDs using the same exe. This is a limited product
  policy, not a universal OS guarantee.
- Remaining exact launch settings can remove identical copies. Labels, icons,
  publishers and version-directory conventions cannot establish identity.
- Steam games match by numeric provider ID, not their name.

Select a current Shell application item, then valid ordinary Start Menu links
(user before shared), then desktop links. At equal rank keep the prior route.
No launch-mode selection UI is added. Invoke the original Shell item/PIDL or
shortcut's default verb with ShellExecuteExW; Steam protocol fallback uses lpFile.
Never launch a resolved exe bare. Failure/cancellation is not hidden by starting
Explorer. No second invocation follows an accepted request.
Only a parsing failure before invocation triggers a fresh catalog scan. Retry
once if that same durable app now has a different locator; never retry a rejected
or cancelled Shell invocation.

The publisher's Shell registration owns update routing. A shortcut is not
immutable: target, name and location can change. Shared identity and saved
aliases preserve placement. There is no Squirrel release parser, updater
command parser, version-directory matcher or application-name exception.

## Durable placement

Reconcile identity/source keys with internal IDs before snapshot diffing.
Existing layout IDs are adopted during migration. Merging retains the first
placement, including a folder position, and preserves hidden intent. New entries
receive a one-time app: ID; Steam keeps its provider ID. Reusing a source path
for a different explicit ID does not inherit the previous application.

user-state.sqlite3, beside the icon cache, stores layout and identity aliases
in one transaction. It retains the initial migration layout and previous
transaction as recovery records. The cache is only the migration input. A corrupt
durable store is reported and never rebuilt/deleted. Icon-cache deletion cannot
remove durable identity or placement.

Failed enumeration protects entries from the affected source; unreadable links
protect their own locators. Removal requires two complete absent observations.
Layout references survive removal so returning apps recover placement. Missing
metadata cannot erase a previously observed explicit identity.
Unavailable Steam libraries and incomplete manifests also report source failure.

## Evidence and verification

tests/fixtures/windows_catalog_observed.json contains sanitized data measured on
the target PC on 2026-09-25. Both Discord links and its Shell item declare
com.squirrel.Discord.Discord, so one tile selects the Shell locator. Creator
Companion's identified and unidentified same-exe links join the Shell item.
Unreal Engine remains discoverable from its desktop-only link. These app names
occur in evidence/tests, not production identity rules.

Tests cover ID conflicts, unidentified bridges, host/argument separation, Steam
IDs, selected-route stability, link moves and version-target changes, folder and
hidden placement, serialization, incomplete scans, actual Shell property reads,
and durable-store backup and icon-cache deletion.

Run the read-only machine report with:
cargo test --bin launchpad-windows report_discovered_app_launchers -- --ignored --nocapture

Optionally set LAUNCHPAD_CATALOG_REPORT to an output JSON path to capture sources,
locators, identity, native-fallback evidence and targets. Real installation paths
remain local; only a sanitized fixture is committed.

## Contracts and limits

- [Application IDs](https://learn.microsoft.com/en-us/windows/win32/shell/appids)
- [Property reads](https://learn.microsoft.com/en-us/windows/win32/api/propsys/nf-propsys-ipropertystore-getvalue)
- [Shell execution](https://learn.microsoft.com/en-us/windows/win32/api/shellapi/ns-shellapi-shellexecuteinfow)

Apps can omit IDs. Arbitrary different executables without shared identity are
not merged by guessing. The observed cases do not require private OS resolver
ABIs. Tests simulate version-target changes and locator replacement; they do not
force a vendor update. Current Shell launch is checked separately on the PC.
