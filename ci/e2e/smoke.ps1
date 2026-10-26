# The CLI smoke flow inside a LabVIEW Windows image: list targets, search,
# check the lock, install, list, uninstall, and check no LabVIEW process ever ran.
# Usage: smoke.ps1 <lvpm binary> <fixture dir> <LabVIEW year the image has>
# LVPM_SMOKE_LABVIEW moves the LabVIEW install dir, for the self-test only.
# smoke.sh is its Linux twin: keep the steps and wording the same.
param(
    [Parameter(Mandatory)] [string] $Lvpm,
    [Parameter(Mandatory)] [string] $Fixture,
    [Parameter(Mandatory)] [int] $Year
)
$ErrorActionPreference = 'Stop'
$pkg = 'oglib_boolean'; $version = '6.0.0.9'
$labview = if ($env:LVPM_SMOKE_LABVIEW) { $env:LVPM_SMOKE_LABVIEW } else { "C:\Program Files\National Instruments\LabVIEW $Year" }
$installed = Join-Path $labview 'user.lib\_OpenG.lib\boolean\boolean.llb'
$total = 7; $script:n = 0; $script:out = ''
$summary = $env:GITHUB_STEP_SUMMARY
$env:LV_RTE_HEADLESS = '1'

$work = Join-Path ([IO.Path]::GetTempPath()) ([IO.Path]::GetRandomFileName())
Copy-Item -Recurse $Fixture (Join-Path $work 'project')
Set-Location (Join-Path $work 'project')

# Sample the process table for LabVIEW while the flow runs.
$sampler = Start-Job {
    while ($true) {
        $p = Get-Process -Name LabVIEW -ErrorAction SilentlyContinue | Select-Object -First 1
        if ($p) { return $p.ProcessName }
        Start-Sleep -Milliseconds 200
    }
}

function Fail([string] $message) {
    Write-Host "::error::$message"
    if ($summary) { Add-Content $summary "$message`n`n``````n$script:out`n``````" }
    exit 1
}

# Step <title> <arguments>: runs the next step, keeps its output in $out.
function Step([string] $title, [string[]] $arguments) {
    $script:n++
    $cmd = "$Lvpm $($arguments -join ' ')"
    Write-Host "::group::step $script:n/${total}: ${title}: $cmd"
    $start = Get-Date
    $ErrorActionPreference = 'Continue'
    # "$_" turns Windows PowerShell's stderr ErrorRecords back into plain lines.
    $script:out = (& $Lvpm @arguments 2>&1 | ForEach-Object { "$_" } | Out-String).TrimEnd()
    $status = $LASTEXITCODE
    Write-Host $script:out
    Write-Host "($([int]((Get-Date) - $start).TotalSeconds)s)"
    Write-Host '::endgroup::'
    if ($status -ne 0) {
        # reqwest's wording for a network failure: the registry, not lvpm.
        if ($script:out -match 'error sending request|dns error|connection refused|timed out|tcp connect') {
            Fail "infrastructure: registry unreachable at step $script:n/${total}: $cmd"
        }
        Fail "FAILED at step $script:n/${total}: $cmd (exit $status)"
    }
}

# Expect <what> <condition>: checks the step that just ran.
function Expect([string] $what, [bool] $ok) {
    if (-not $ok) { Fail "FAILED at step $script:n/${total}: expected $what" }
}

try {
    Step 'targets' @('targets')
    Expect "LabVIEW $Year in the targets" ($script:out -match "LabVIEW $Year")
    Step 'search' @('search', $pkg)
    Expect "$pkg in the search" ($script:out -match "(?m)^$pkg ")
    Step 'lock check' @('lock', '--check')
    Step 'install' @('install')
    Expect "$installed to exist" (Test-Path $installed)
    Step 'list' @('list')
    Expect "$pkg $version in the list" ($script:out -match "(?m)^$pkg +$([regex]::Escape($version)) ")
    Step 'uninstall' @('uninstall', $pkg)
    Expect "$installed to be gone" (-not (Test-Path $installed))
    $script:n++; $script:out = ''
    # Running: never saw LabVIEW. Completed: saw it. Anything else: no check ran.
    if ($sampler.State -notin 'Running', 'Completed') { Fail "FAILED at step $script:n/${total}: the LabVIEW sampler stopped ($($sampler.State))" }
    $seen = if ($sampler.State -eq 'Completed') { Receive-Job $sampler }
    if ($seen) { Fail "FAILED at step $script:n/${total}: LabVIEW was started ($seen)" }
} finally {
    Remove-Job $sampler -Force; Set-Location $env:TEMP; Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}
Write-Host "smoke flow passed all $total steps (LabVIEW $Year)"
