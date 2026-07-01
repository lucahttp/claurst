@echo off
cd /d C:\Users\lucas\claurst
git remote add upstream https://github.com/Kuberwastaken/claurst.git
git fetch upstream
git fetch origin
git log --oneline -20
echo === UPSTREAM MAIN ===
git log --oneline upstream/main -10
echo === STATUS ===
git status
