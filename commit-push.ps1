@echo off
cd /d C:\Users\lucas\claurst
echo === Git Status ===
git status
echo.
echo === Add and Commit ===
git add -A
git commit -m "feat(plugins): add GitHub-based marketplace support

- Add /plugin marketplace add <source> command for adding marketplaces by
  GitHub shorthand (owner/repo), URL, or local path
- Add /plugin marketplace list, search, remove commands
- Add /plugin marketplace <plugin@marketplace> for installing plugins
- Implement MarketplaceSource parsing with support for:
  - github shorthand (owner/repo[@ref])
  - git SSH URLs (git@host:owner/repo.git)
  - git HTTPS URLs
  - Direct URLs
  - Local file and directory paths
- Add MarketplaceManifest and PluginMarketplaceEntry types
- Add known_marketplaces.json persistence
- Cache marketplaces in ~/.claurst/plugins/marketplaces/
- Cache plugins in ~/.claurst/plugins/cache/<mkt>/<plugin>/
- Keep backward compatibility with old registry.claude.ai API
- Update docs with new marketplace commands

Based on Claude Code plugin marketplace implementation analysis."
echo.
echo === Push ===
git push origin main
echo.
echo Done.
pause
