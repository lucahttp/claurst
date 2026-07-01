# Sync and merge script for claurst
# Run this in PowerShell: powershell -File C:\Users\lucas\claurst\sync-and-merge.ps1

$ErrorActionPreference = "Continue"
$repoPath = "C:\Users\lucas\claurst"

Write-Host "=== Step 1: Add upstream remote ===" -ForegroundColor Cyan
Set-Location $repoPath
git remote add upstream https://github.com/Kuberwastaken/claurst.git 2>&1
git remote -v

Write-Host "`n=== Step 2: Fetch upstream ===" -ForegroundColor Cyan
git fetch upstream
git fetch origin

Write-Host "`n=== Step 3: Current status ===" -ForegroundColor Cyan
git log --oneline -5
Write-Host "`n--- upstream/main ---"
git log --oneline upstream/main -5

Write-Host "`n=== Step 4: Merge upstream/main into main ===" -ForegroundColor Cyan
git checkout main
git merge upstream/main --no-edit

Write-Host "`n=== Step 5: Push to origin ===" -ForegroundColor Cyan
git push origin main

Write-Host "`n=== Done! ===" -ForegroundColor Green
