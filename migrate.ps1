#Requires -Version 5.1

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$WorkspaceRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$TempDir       = Join-Path ([System.IO.Path]::GetTempPath()) "monorepo-migrate-$(Get-Random)"
$SubRepos      = @('ic20-lsp', 'ic20-vscode', 'ic20c')

function Invoke-Git([string[]]$Arguments) {
    & git @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "git $($Arguments -join ' ') exited $LASTEXITCODE"
    }
}

function Get-DefaultBranch([string]$RepoPath) {
    $branch = (& git -C $RepoPath rev-parse --abbrev-ref HEAD 2>&1) | Out-String
    if ($LASTEXITCODE -ne 0) { throw "Cannot determine default branch for: $RepoPath" }
    return $branch.Trim()
}

Set-Location $WorkspaceRoot

if (Test-Path '.git') {
    Write-Error "A .git directory already exists at the workspace root. Aborting."; exit 1
}

foreach ($repo in $SubRepos) {
    if (-not (Test-Path "$repo\.git")) {
        Write-Error "Expected '$repo\.git' but it was not found. Aborting."; exit 1
    }
}

Write-Host ""
Write-Host "Monorepo migration plan:"
Write-Host "  Workspace : $WorkspaceRoot"
Write-Host "  Merge     : $($SubRepos -join ', ')  [full history via git subtree]"
Write-Host "  Plain add : ic10-lsp  [no prior history]"
Write-Host "  Temp dir  : $TempDir"
Write-Host ""
Write-Host "The .git directories in each sub-repo will be permanently removed." -ForegroundColor Yellow
Write-Host "Temporary clones are made first, so no history is lost."            -ForegroundColor Yellow
Write-Host ""
$answer = Read-Host "Type 'yes' to proceed"
if ($answer -ne 'yes') { Write-Host "Aborted."; exit 0 }

Write-Host ""
Write-Host "[1/5] Creating temporary clones..." -ForegroundColor Cyan
New-Item -ItemType Directory -Path $TempDir | Out-Null

$cloneInfo = @{}
foreach ($repo in $SubRepos) {
    $dest   = Join-Path $TempDir $repo
    $branch = Get-DefaultBranch -RepoPath $repo
    Write-Host "  $repo  (branch: $branch)"
    Invoke-Git 'clone', '--local', (Resolve-Path $repo).Path, $dest
    $cloneInfo[$repo] = @{ Path = $dest; Branch = $branch }
}

Write-Host ""
Write-Host "[2/5] Initializing root repository..." -ForegroundColor Cyan
Invoke-Git 'init'

Write-Host ""
Write-Host "[3/5] Committing root-level files and ic10-lsp..." -ForegroundColor Cyan
# Exclude sub-repos: their nested .git dirs would generate a submodule warning.
# ic10-lsp has no .git so it stages normally here.
Get-ChildItem -Path $WorkspaceRoot -Force |
    Where-Object { $_.Name -notin ($SubRepos + @('.git')) } |
    ForEach-Object { Invoke-Git 'add', '--', $_.Name }
Invoke-Git 'commit', '-m', 'initialize monorepo root'

Write-Host ""
Write-Host "[4/5] Merging sub-repo histories (git subtree add)..." -ForegroundColor Cyan
Write-Host "      Nested .git dirs mask sub-repo content from root git during this step,"
Write-Host "      preventing untracked-file conflicts during the merge."
foreach ($repo in $SubRepos) {
    $info = $cloneInfo[$repo]
    Write-Host "  Merging $repo..."
    Invoke-Git 'subtree', 'add', "--prefix=$repo", $info.Path, $info.Branch
}

Write-Host ""
Write-Host "[5/5] Removing nested .git directories..." -ForegroundColor Cyan
foreach ($repo in $SubRepos) {
    Write-Host "  Removing $repo\.git"
    Remove-Item -Recurse -Force (Join-Path $repo '.git')
}

Write-Host ""
Write-Host "Cleaning up temp clones..."
Remove-Item -Recurse -Force $TempDir

Write-Host ""
Write-Host "Migration complete!" -ForegroundColor Green
Write-Host ""
Write-Host "Review the result:"
Write-Host "  git log --oneline --graph --all"
Write-Host "  git status"
Write-Host ""
Write-Host "When satisfied, add a remote and push:"
Write-Host "  git remote add origin <url>"
Write-Host "  git push -u origin HEAD"
