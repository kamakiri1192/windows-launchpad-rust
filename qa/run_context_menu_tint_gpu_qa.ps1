$ErrorActionPreference = 'Stop'

$handlerPath = '.\src\app\handler.rs'
$handler = Get-Content $handlerPath -Raw
$waitUntil = @'
        if let Some(deadline) = self.qa_next_deadline() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline.max(now)));
        } else {
'@
$pollQa = @'
        if self.qa_enabled() {
            // Temporary visual-QA override: Windows can coalesce WaitUntil
            // wakeups for a hidden window, so poll for this short scenario.
            event_loop.set_control_flow(ControlFlow::Poll);
        } else if let Some(deadline) = self.qa_next_deadline() {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline.max(now)));
        } else {
'@
if (-not $handler.Contains($waitUntil)) {
    throw 'QA control-flow anchor was not found'
}
Set-Content $handlerPath ($handler.Replace($waitUntil, $pollQa)) -NoNewline

cargo build --locked
if ($LASTEXITCODE -ne 0) {
    throw "cargo build failed with code $LASTEXITCODE"
}

$env:LAUNCHPAD_ALLOW_SCREENSHOT = '1'
$env:LAUNCHPAD_QA_SCENARIO = (Resolve-Path .\qa\context_menu_tint.json).Path
$process = Start-Process -FilePath .\target\debug\launchpad-windows.exe -PassThru
if (-not $process.WaitForExit(30000)) {
    $process.Kill()
    throw 'GPU QA scenario timed out after 30 seconds'
}
if ($process.ExitCode -ne 0) {
    throw "GPU QA scenario exited with code $($process.ExitCode)"
}

$runDir = Get-ChildItem .\target\qa-sequences -Directory |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1
if (-not $runDir) {
    throw 'GPU QA did not create an output directory'
}

$artifactDir = '.\target\qa-artifact\context-menu-tint'
New-Item -ItemType Directory -Force -Path $artifactDir | Out-Null
Copy-Item (Join-Path $runDir.FullName '*') $artifactDir -Recurse -Force

$manifestPath = Join-Path $runDir.FullName 'manifest.json'
$manifest = Get-Content $manifestPath -Raw | ConvertFrom-Json
$openFrames = @($manifest.frames | Where-Object {
    $_.context_menu_active -eq $true -and $_.context_menu_phase -eq 'Open'
})
if ($openFrames.Count -eq 0) {
    throw 'GPU QA captured no frame with context_menu_active=true and phase=Open'
}

$selected = $openFrames | Select-Object -Last 1
$selectedPath = Join-Path $runDir.FullName $selected.file
New-Item -ItemType Directory -Force -Path .\docs\qa | Out-Null
Copy-Item $selectedPath .\docs\qa\context-menu-tint.png -Force

Write-Host "Selected verified frame: $($selected.file)"
Write-Host "Open context-menu frames: $($openFrames.Count)"

git restore src/app/handler.rs
git config user.name 'github-actions[bot]'
git config user.email '41898282+github-actions[bot]@users.noreply.github.com'
git add docs/qa/context-menu-tint.png
git rm .github/workflows/context-menu-tint-visual-qa.yml
git rm .github/workflows/context-menu-tint-visual-qa-poll.yml
git rm .github/workflows/context-menu-tint-gpu-qa-final.yml
git rm qa/context_menu_tint_qa.trigger
git rm qa/run_context_menu_tint_gpu_qa.ps1
git commit -m 'Capture verified context menu tint GPU QA'
git push origin HEAD:agent/fix-glass-surface-tint
if ($LASTEXITCODE -ne 0) {
    throw "git push failed with code $LASTEXITCODE"
}
