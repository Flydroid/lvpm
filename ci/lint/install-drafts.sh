#!/usr/bin/env bash
# Checks that install.ps1 skips a draft release: extracts Select-Release with
# the PowerShell parser (the script itself is not run) and feeds it a releases
# list holding a draft v0.4.0, a published pre-release v0.3.0 and a published
# stable v0.2.1. Also checks the script picks its release through it.
# Usage: install-drafts.sh  (run from the repo root; PWSH overrides the shell)
set -euo pipefail
"${PWSH:-pwsh}" -NoProfile -NonInteractive -Command '
  $ErrorActionPreference = "Stop"
  $ast = [System.Management.Automation.Language.Parser]::ParseFile((Resolve-Path "scripts/install.ps1"), [ref]$null, [ref]$null)
  $fn = $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.FunctionDefinitionAst] -and $n.Name -eq "Select-Release" }, $true) | Select-Object -First 1
  if (-not $fn) { Write-Host "install drafts check: Select-Release not found in scripts/install.ps1"; exit 1 }
  $calls = $ast.FindAll({ param($n) $n -is [System.Management.Automation.Language.CommandAst] -and $n.GetCommandName() -eq "Select-Release" }, $true)
  if (-not $calls) { Write-Host "install drafts check: scripts/install.ps1 never calls Select-Release"; exit 1 }
  . ([scriptblock]::Create($fn.Extent.Text))
  $releases = Get-Content -Raw "ci/fixtures/releases-draft.json" | ConvertFrom-Json
  $failed = $false
  foreach ($case in @(@{ Stable = $false; Want = "v0.3.0" }, @{ Stable = $true; Want = "v0.2.1" })) {
    $picked = (Select-Release $releases $case.Stable).tag_name
    if ($picked -ne $case.Want) { Write-Host "install drafts check: -Stable $($case.Stable) picked $picked, expected $($case.Want)"; $failed = $true }
  }
  if ($failed) { exit 1 }
  Write-Host "install drafts check passed: draft v0.4.0 skipped; picked v0.3.0, and v0.2.1 with -Stable"
'
