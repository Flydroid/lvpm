# Self-test for smoke.ps1, with a stub lvpm: the same scenarios as smoke.test.sh.
$ErrorActionPreference = 'Stop'
$here = $PSScriptRoot
# The flow fails on any LabVIEW process, so an open one fails every scenario.
if (Get-Process LabVIEW -ErrorAction SilentlyContinue) { 'close LabVIEW before running this self-test'; exit 1 }
$work = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
New-Item -ItemType Directory $work | Out-Null
$failures = 0
$llbRel = 'user.lib\_OpenG.lib\boolean\boolean.llb'

# A stub lvpm. STUB_FAIL names the subcommand that fails; STUB_LABVIEW=1 starts
# a process named LabVIEW during install; STUB_OFFLINE=1 makes search fail the
# way reqwest does without a network; STUB_NOFILE=1 makes install write nothing.
$stub = Join-Path $work 'lvpm.ps1'
@'
$llb = Join-Path $env:LVPM_SMOKE_LABVIEW 'user.lib\_OpenG.lib\boolean\boolean.llb'
if ($args[0] -eq $env:STUB_FAIL) { "stub: $($args[0]) failed"; exit 3 }
switch ($args[0]) {
  'targets' { 'LabVIEW 2026 (64-bit)  v26  C:\Program Files\National Instruments\LabVIEW 2026' }
  'search'  { if ($env:STUB_OFFLINE) { 'Error: fetching http://x: error sending request'; exit 1 }
              'oglib_boolean   6.0.0.9   OpenG Boolean Library' }
  'lock'    { 'lvpm.lock is up to date' }
  'install' { if (-not $env:STUB_NOFILE) { New-Item -ItemType Directory -Force $llb | Out-Null }
              if ($env:STUB_LABVIEW) {
                $exe = Join-Path $env:STUB_WORK 'LabVIEW.exe'
                Copy-Item "$env:SystemRoot\System32\PING.EXE" $exe
                Start-Process $exe '-n 6 127.0.0.1' -WindowStyle Hidden; Start-Sleep 3
              }
              'done' }
  'list'    { 'oglib_boolean   6.0.0.9   5 files' }
  'uninstall' { Remove-Item -Recurse -Force $llb; 'removed' }
}
exit 0
'@ | Set-Content -Encoding utf8 $stub

function Expect-Run([int] $want, [string] $name, [string] $msg, [hashtable] $env = @{}) {
    $lv = Join-Path $work 'LabVIEW 2026'
    Remove-Item -Recurse -Force $lv -ErrorAction SilentlyContinue
    $summary = Join-Path $work 'summary'; Set-Content $summary ''
    $vars = @{ LVPM_SMOKE_LABVIEW = $lv; STUB_WORK = $work; GITHUB_STEP_SUMMARY = $summary
               STUB_FAIL = ''; STUB_LABVIEW = ''; STUB_OFFLINE = ''; STUB_NOFILE = '' }
    foreach ($k in $env.Keys) { $vars[$k] = $env[$k] }
    foreach ($k in $vars.Keys) { Set-Item "env:$k" $vars[$k] }
    $out = & powershell -NoProfile -File (Join-Path $here 'smoke.ps1') $stub (Join-Path $here 'fixture') 2026 2>&1 | ForEach-Object { "$_" }
    $got = $LASTEXITCODE
    $last = $out | Select-Object -Last 1
    $inSummary = ($want -eq 0) -or ((Get-Content -Raw $summary) -like "*$msg*")
    if ($got -eq $want -and $last -like "*$msg*" -and $inSummary) { "ok   $name" }
    else { "FAIL $name (exit $got): $($out -join "`n")"; $script:failures++ }
}

Expect-Run 0 'the flow passes' 'smoke flow passed all 7 steps (LabVIEW 2026)'
Expect-Run 1 'a failing lock check stops at step 3' "FAILED at step 3/7: $stub lock --check (exit 3)" @{ STUB_FAIL = 'lock' }
Expect-Run 1 'a LabVIEW process fails the flow' 'FAILED at step 7/7: LabVIEW was started (LabVIEW)' @{ STUB_LABVIEW = '1' }
Expect-Run 1 'an unreachable registry is infrastructure' 'infrastructure: registry unreachable at step 2/7' @{ STUB_OFFLINE = '1' }
Expect-Run 1 'a missing package file fails install' "FAILED at step 4/7: expected $work\LabVIEW 2026\$llbRel to exist" @{ STUB_NOFILE = '1' }

# Only the stub's own LabVIEW, never one the developer has open.
Get-Process LabVIEW -ErrorAction SilentlyContinue | Where-Object { $_.Path -and $_.Path.Contains((Split-Path $work -Leaf)) } | Stop-Process -Force
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
if ($failures) { "$failures scenario(s) failed"; exit 1 } else { 'all smoke scenarios passed' }
