# Windows application discovery

The application catalog combines these sources:

- Per-user and shared Start Menu folders, recursively, excluding the Startup
  known folders.
- Per-user and shared desktop known folders, at the top level only. Known Folder
  resolution also covers redirected desktops such as OneDrive.
- Packaged applications from `shell:AppsFolder` and installed Steam manifests.

Filesystem discovery accepts `.lnk` shortcuts whose resolved target is an `.exe`
or `.com`. Document links and internet shortcuts are not imported by this path.
This is an application catalog, not a copy of Windows Search's full index.

## Duplicate shortcuts

Shortcut equivalence and application identity are different questions. Two
shortcuts can be collapsed when the following known launch settings match:

- Absolute target path.
- Arguments, compared exactly, including case and whitespace.
- Working directory, without repairing invalid values or assuming defaults.
- Elevation and separate-process flags.
- Initial window display mode.

Path comparison normalizes ASCII case and slash direction. It does not infer
equivalence from executable names, product names, versions, or directory layout.
Missing metadata and flags requiring unsupported extra data (such as installer
activation or compatibility shims) prevent deduplication. Unknown flags also
prevent deduplication.

Equivalent entries retain the lowest existing `AppId` in lexical order, making
selection independent of enumeration order. No new application ID is invented.

An updater executable and a versioned application executable are distinct launch
routes. They remain separate even if a person knows they belong to the same app.
Combining them requires a separate application-identity design with evidence for
both equivalence and the preferred launch route; per-application name exceptions
are not part of shortcut deduplication.

## Verification

`cargo test --all-targets` covers launch-key comparisons and real, temporary Shell
links with different elevation/display settings. The folder-walk test covers
Startup exclusion and nonrecursive desktop discovery.

For a read-only report of this machine's catalog:

```powershell
cargo test --bin launchpad-windows report_discovered_app_launchers -- --ignored --nocapture
```

The diagnostic enumerates applications without launching them or changing their
shortcuts. Its output varies with the machine and is not a portable test fixture.
