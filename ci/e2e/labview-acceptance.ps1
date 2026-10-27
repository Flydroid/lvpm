# Runs the @labview acceptance scenarios against the LabVIEW in a Windows image.
# Usage: labview-acceptance.ps1 -Image <image> -Year <year>   (on the runner; starts the container)
#        labview-acceptance.ps1 -Year <year> -InContainer     (inside it; run by the line above)
# The tests are built at C:\src\lvpm and mounted at the same path, because
# CARGO_MANIFEST_DIR and CARGO_BIN_EXE_lvpm are fixed at compile time.
param([string] $Image, [Parameter(Mandatory)] [int] $Year, [switch] $InContainer)
$ErrorActionPreference = 'Stop'
$src = 'C:\src\lvpm'

if (-not $InContainer) {
    # The step summary goes to out\, which the workflow copies out afterwards.
    New-Item -ItemType Directory -Force out | Out-Null
    docker run --rm --isolation=process --entrypoint powershell -e GITHUB_STEP_SUMMARY=C:\out\summary.md `
        -v "${src}:${src}" -v "${PSScriptRoot}:C:\e2e:ro" -v "${PWD}\out:C:\out" $Image `
        -NoProfile -ExecutionPolicy Bypass -File C:\e2e\labview-acceptance.ps1 -Year $Year -InContainer
    exit $LASTEXITCODE
}

function Fail([string] $message, [string] $log = '') {
    Write-Host "::error::$message"
    # A step summary is capped at 1 MiB: keep the tail, where the failure and the counts are.
    $tail = ($log -split "`n" | Select-Object -Last 200) -join "`n"
    if ($env:GITHUB_STEP_SUMMARY) { Add-Content $env:GITHUB_STEP_SUMMARY "$message`n`n``````n$tail`n``````" }
    exit 1
}

# The plan leaves starting LabVIEW to the scenarios. This script starts one up
# front on purpose, so a slow first start is not charged to the first scenario;
# the container is thrown away afterwards.
$labview = "C:\Program Files\National Instruments\LabVIEW $Year"
$ini = Join-Path $labview 'LabVIEW.ini'
# NI's image may set no VI Server port, leaving LabVIEW on its own default
# while lvpm and the test harness fall back to 3363. Pin the port in this
# throwaway container's LabVIEW.ini so all three agree, as a developer's
# machine does.
$port = 3363
if (-not (Test-Path (Join-Path $labview 'LabVIEW.exe'))) { Fail "infrastructure: no LabVIEW.exe under $labview" }
$lines = if (Test-Path $ini) { @(Get-Content $ini | Where-Object { $_ -notmatch '^\s*server\.tcp\.(port|enabled)\s*=' }) } else { @() }
if ($lines -notcontains '[LabVIEW]') { $lines = @('[LabVIEW]') + $lines }
$pinned = foreach ($l in $lines) { $l; if ($l -eq '[LabVIEW]') { 'server.tcp.enabled=True'; "server.tcp.port=$port" } }
Set-Content -Path $ini -Value $pinned -Encoding ascii
# Without NI's headless switch a container's LabVIEW blocks on an activation
# prompt nobody sees and never opens VI Server (CI, 2026-10-26: no port at
# all). Only this LabVIEW gets it: the harness picks its children's per scenario.
$env:LV_RTE_HEADLESS = '1'
Start-Process (Join-Path $labview 'LabVIEW.exe')
Remove-Item Env:LV_RTE_HEADLESS
$deadline = (Get-Date).AddMinutes(5)
while (-not (Test-NetConnection 127.0.0.1 -Port $port -InformationLevel Quiet -WarningAction SilentlyContinue)) {
    # By name, not by the started process, in case LabVIEW.exe hands off and
    # exits (not measured in the image; a renamed child would fail here).
    if (-not (Get-Process LabVIEW -ErrorAction SilentlyContinue)) { Fail "infrastructure: LabVIEW exited before VI Server answered on port $port" }
    if ((Get-Date) -gt $deadline) {
        # Where LabVIEW does listen, if anywhere, says whether the port or the start is wrong.
        $listening = (Get-Process LabVIEW | ForEach-Object { Get-NetTCPConnection -OwningProcess $_.Id -State Listen -ErrorAction SilentlyContinue } | ForEach-Object LocalPort) -join ', '
        $dialogs = Get-ChildItem "$env:TEMP\LabVIEW_*_headless_*_cur.txt" -ErrorAction SilentlyContinue | Get-Content -Raw
        Fail "infrastructure: LabVIEW did not answer VI Server on port $port within 5 minutes (LabVIEW listens on: $(if ($listening) { $listening } else { 'nothing' }))" "$dialogs"
    }
    Start-Sleep 5
}

$binary = Get-ChildItem "$src\target\debug\deps\acceptance-*.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
if (-not $binary) { Fail "no acceptance test binary under $src\target\debug\deps" }
$env:LVPM_TEST_LABVIEW = '1'; $env:LVPM_TEST_ONLY_LABVIEW = '1'; $env:LVPM_TEST_LABVIEW_HEADLESS = '1'
$ErrorActionPreference = 'Continue'
$log = (& $binary.FullName 2>&1 | ForEach-Object { "$_" } | Out-String)
$status = $LASTEXITCODE
Write-Host $log

# A scenario only a headless LabVIEW runs is recorded where one is: here. Record
# the missing ones (existing ones are kept) and hand them out to be committed.
if ($log -match 'no recording `') {
    $golden = "$src\tests\golden"
    $before = @(Get-ChildItem $golden -Directory | ForEach-Object Name)
    $env:LVPM_RECORD = '1'; & $binary.FullName *> $null; Remove-Item Env:LVPM_RECORD
    $new = @(Get-ChildItem $golden -Directory | Where-Object { $before -notcontains $_.Name })
    New-Item -ItemType Directory -Force C:\out\golden | Out-Null
    foreach ($d in $new) { Copy-Item $d.FullName -Destination C:\out\golden\$($d.Name) -Recurse -Force }
    $shown = foreach ($d in $new) { foreach ($f in Get-ChildItem $d.FullName -File) { "==> $($d.Name)/$($f.Name)"; Get-Content $f.FullName -Raw } }
    Fail "missing recordings, recorded here: $($new.Name -join ', ') (the golden artifact holds them, to commit)" ($shown -join "`n")
}

if ($status -ne 0) { Fail "the @labview scenarios failed (exit $status)" $log }
# The harness counts every @labview scenario in FEATURE_FILES as run or not run
# (with the reason), so "none not run" plus "some run" is the full check; no
# hand-kept total to bump when a slice adds scenarios.
if ($log -match '@labview scenarios not run') { Fail 'some @labview scenarios were not run' $log }
if ($log -notmatch '(?m)^([1-9]\d*) @labview scenarios run') { Fail 'no @labview scenarios ran' $log }
$ran = $Matches[1]
if ($log -notmatch '@unsafe scenarios never run') { Fail 'expected the @unsafe scenarios to be reported as never run' $log }
Write-Host "$ran @labview scenarios passed against LabVIEW $Year"
