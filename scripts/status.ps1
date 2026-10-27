#Requires -Version 5.1
<#
.SYNOPSIS
    Show which worktree works on what, and which slices can be built next.

.DESCRIPTION
    Lists every git worktree with its branch, uncommitted changes, last
    commit and the pull request of that branch. A worktree whose PR is merged
    or closed, that has no uncommitted changes and whose HEAD is still the PR
    head commit is marked "stale".

    Then lists the open slice issues (label "slice") that have no open
    blockers and no sub-issues (so the parent issue is left out): the slices
    that can be built next. The full picture is the "lvpm mission" project:
    https://github.com/users/ThatsTheEnd/projects/2

    Needs git and an authenticated gh.
#>
$ErrorActionPreference = 'Stop'

# Fork PRs can reuse a local branch name; only this repo's branches count.
$prs = (gh pr list --state all --limit 1000 --json headRefName,headRefOid,number,state,title,isCrossRepository | ConvertFrom-Json) | Where-Object { -not $_.isCrossRepository }

$rows = @()
$worktree = $null
foreach ($line in (git worktree list --porcelain) + '') {
    if ($line -like 'worktree *') { $worktree = @{ Path = $line -replace '^worktree ', ''; Branch = '(detached)' } }
    elseif ($line -like 'branch refs/heads/*') { $worktree.Branch = $line -replace '^branch refs/heads/', '' }
    elseif ($line -eq '' -and $worktree) {
        # Agent metrics and review reports change in every worktree; they are not work in progress.
        $uncommitted = @(git -C $worktree.Path status --porcelain -- . ':!.claude/metrics' ':!.dev-team-reports').Count
        $lastCommitAge = git -C $worktree.Path log -1 --format=%cr
        # The newest PR of the branch; a branch name can be reused.
        $pr = $prs | Where-Object headRefName -eq $worktree.Branch | Sort-Object number -Descending | Select-Object -First 1
        $prText = if ($pr) { "#$($pr.number) $($pr.state) $($pr.title)" } else { '(no PR)' }
        # HEAD must still be the PR's head: new commits on a reused branch are work in progress.
        $stale = $pr -and $pr.state -ne 'OPEN' -and $uncommitted -eq 0 -and (git -C $worktree.Path rev-parse HEAD) -eq $pr.headRefOid
        $rows += [pscustomobject]@{
            Worktree    = Split-Path $worktree.Path -Leaf
            Branch      = $worktree.Branch
            Uncommitted = $uncommitted
            LastCommit  = $lastCommitAge
            PR          = $prText
            Note        = if ($stale) { 'stale' } else { '' }
        }
        $worktree = $null
    }
}
'== Worktrees'
$rows | Format-Table -AutoSize | Out-String -Width 200

'== Ready to build (open slices, no open blockers)'
# ConvertFrom-Json emits each page's array as one object; ForEach-Object unrolls it.
$issues = gh api --paginate 'repos/{owner}/{repo}/issues?labels=slice&state=open&per_page=100' | ConvertFrom-Json | ForEach-Object { $_ }
$ready = $issues | Where-Object {
    -not $_.pull_request -and $_.issue_dependencies_summary.blocked_by -eq 0 -and $_.sub_issues_summary.total -eq 0
}
$ready | Sort-Object number | ForEach-Object { "  #$($_.number)  $($_.title)  [$($_.milestone.title)]" }
