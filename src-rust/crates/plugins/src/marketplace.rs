//! Plugin marketplace — mirrors src/commands/plugin/ (17 files).
//!
//! Provides search, install, update, list, and uninstall for plugins
//! from the Claude registry.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A plugin entry from the marketplace registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceEntry {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub download_url: String,
    pub hash: String,
    pub tags: Vec<String>,
    pub updated_at: Option<u64>,
}

/// An installed plugin summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    pub version: String,
    pub install_path: std::path::PathBuf,
    pub description: String,
}

// ---------------------------------------------------------------------------
// Marketplace API client
// ---------------------------------------------------------------------------

const REGISTRY_URL: &str = "https://registry.claude.ai/plugins";

/// Search the marketplace for plugins matching `query`, optionally filtered by `tags`.
///
/// When `tags` is non-empty, `tags[]=tag` query parameters are appended to the URL.
pub async fn marketplace_search_filtered(
    query: &str,
    tags: &[&str],
) -> Result<Vec<MarketplaceEntry>, String> {
    let mut params: Vec<String> = Vec::new();

    if !query.is_empty() {
        params.push(format!("q={}", urlencoding::encode(query)));
    }
    for tag in tags {
        params.push(format!("tags[]={}", urlencoding::encode(tag)));
    }

    let url = if params.is_empty() {
        REGISTRY_URL.to_string()
    } else {
        format!("{}?{}", REGISTRY_URL, params.join("&"))
    };

    let resp = reqwest::get(&url)
        .await
        .map_err(|e| format!("HTTP error: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Registry returned {}", resp.status()));
    }

    resp.json::<Vec<MarketplaceEntry>>()
        .await
        .map_err(|e| format!("Parse error: {e}"))
}

/// Search the marketplace for plugins matching `query`.
///
/// Convenience wrapper around [`marketplace_search_filtered`] with no tag filter.
pub async fn marketplace_search(query: &str) -> Result<Vec<MarketplaceEntry>, String> {
    marketplace_search_filtered(query, &[]).await
}

/// Check all installed plugins for updates.
///
/// Returns `(name, current_version, latest_version)` for each plugin that has
/// a newer version available in the marketplace.
pub async fn marketplace_check_updates_all() -> Vec<(String, String, String)> {
    let installed = list_installed();
    let mut futures_vec = Vec::new();
    for plugin in &installed {
        let name = plugin.name.clone();
        let current = plugin.version.clone();
        futures_vec.push(async move {
            let results = marketplace_search(&name).await.unwrap_or_default();
            if let Some(latest) = results.iter().find(|e| e.name == name) {
                if latest.version != current {
                    return Some((name, current, latest.version.clone()));
                }
            }
            None
        });
    }
    futures::future::join_all(futures_vec)
        .await
        .into_iter()
        .flatten()
        .collect()
}

/// Install a plugin by name from the marketplace, from a GitHub repo, or from a URL directly.
pub async fn marketplace_install(name_or_url: &str) -> Result<InstalledPlugin, String> {
    let entry = if name_or_url.starts_with("http") {
        // Direct URL install
        MarketplaceEntry {
            name: name_or_url
                .split('/')
                .last()
                .unwrap_or("plugin")
                .trim_end_matches(".zip")
                .to_string(),
            version: "0.0.0".to_string(),
            description: String::new(),
            author: String::new(),
            download_url: name_or_url.to_string(),
            hash: String::new(),
            tags: Vec::new(),
            updated_at: None,
        }
    } else if name_or_url.contains('/') && !name_or_url.starts_with("http") {
        // GitHub shorthand: "owner/repo" or "owner/repo@tag"
        let (repo, tag) = if let Some((owner_repo, version)) = name_or_url.split_once('@') {
            (owner_repo.to_string(), Some(version.to_string()))
        } else {
            (name_or_url.to_string(), None)
        };

        install_from_github(&repo, tag.as_deref()).await?
    } else {
        // Search by name in the registry
        let results = marketplace_search(name_or_url).await?;
        results
            .into_iter()
            .find(|e| e.name == name_or_url)
            .ok_or_else(|| format!("Plugin '{}' not found in marketplace", name_or_url))?
    };

    let install_dir = plugin_install_dir(&entry.name);
    std::fs::create_dir_all(&install_dir).map_err(|e| format!("Create dir: {e}"))?;

    // Download archive
    let resp = reqwest::get(&entry.download_url)
        .await
        .map_err(|e| format!("Download error: {e}"))?;
    let bytes = resp.bytes().await.map_err(|e| format!("Read bytes: {e}"))?;

    // Verify hash if provided
    if !entry.hash.is_empty() {
        use sha2::{Digest, Sha256};
        let computed = hex::encode(Sha256::digest(&bytes));
        if computed != entry.hash {
            return Err(format!(
                "Hash mismatch: expected {}, got {}",
                entry.hash, computed
            ));
        }
    }

    // Write to disk (assume .zip or direct .yaml file)
    let archive_path = install_dir.join("plugin.zip");
    std::fs::write(&archive_path, &bytes).map_err(|e| format!("Write: {e}"))?;

    // Try to unzip; if not a zip, treat as manifest YAML
    if let Err(e) = try_unzip_flat(&archive_path, &install_dir) {
        // If unzip failed (not a zip), assume it's the manifest directly
        let manifest_path = install_dir.join("manifest.yaml");
        std::fs::copy(&archive_path, &manifest_path).map_err(|e| format!("Copy: {}", e))?;
        let _ = std::fs::remove_file(&archive_path);
    } else {
        let _ = std::fs::remove_file(&archive_path);
    }

    Ok(InstalledPlugin {
        name: entry.name.clone(),
        version: entry.version.clone(),
        install_path: install_dir,
        description: entry.description.clone(),
    })
}

/// Install a plugin directly from a GitHub repository (public API).
///
/// Handles:
/// - `owner/repo` — default branch archive
/// - `owner/repo@branch` — specific branch archive
/// - `owner/repo@tag` — GitHub release (tries release first, falls back to branch)
///
/// This function returns a MarketplaceEntry with a download URL. The actual
/// download and extraction is done by the caller (marketplace_install).
pub async fn install_from_github(repo: &str, tag: Option<&str>) -> Result<MarketplaceEntry, String> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid GitHub repo format: '{}'. Expected 'owner/repo'", repo));
    }
    let (owner, repo_name) = (parts[0], parts[1]);

    // First, get the default branch name
    let default_branch = get_default_branch(owner, repo_name).await?;

    let download_url = if let Some(tag) = tag {
        // Check if it's a release tag first
        if let Ok(url) = try_get_release_download_url(owner, repo_name, tag).await {
            url
        } else {
            // Fall back to branch archive
            format!(
                "https://github.com/{}/{}/archive/refs/heads/{}.zip",
                owner, repo_name, tag
            )
        }
    } else {
        // Use default branch archive
        format!(
            "https://github.com/{}/{}/archive/refs/heads/{}.zip",
            owner, repo_name, default_branch
        )
    };

    Ok(MarketplaceEntry {
        name: repo_name.to_string(),
        version: tag.unwrap_or(&default_branch).to_string(),
        description: format!("Installed from {}/{}", owner, repo_name),
        author: owner.to_string(),
        download_url,
        hash: String::new(),
        tags: vec!["github".to_string()],
        updated_at: None,
    })
}

/// Get the default branch name of a GitHub repository.
async fn get_default_branch(owner: &str, repo: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let url = format!("https://api.github.com/repos/{}/{}", owner, repo);

    let resp = client
        .get(&url)
        .header("User-Agent", "Claurst/1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub API request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {} for repo {}/{}", resp.status(), owner, repo));
    }

    let data: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse GitHub response: {}", e))?;

    let branch = data.get("default_branch")
        .and_then(|b| b.as_str())
        .unwrap_or("main")
        .to_string();

    Ok(branch)
}

/// Try to get download URL from a GitHub release (specific tag).
async fn try_get_release_download_url(owner: &str, repo: &str, tag: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let release_url = format!("https://api.github.com/repos/{}/{}/releases/tags/{}", owner, repo, tag);

    let resp = client
        .get(&release_url)
        .header("User-Agent", "Claurst/1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub API request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Release not found: {}", resp.status()));
    }

    let data: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse release response: {}", e))?;

    // Try to find a zip asset first
    if let Some(assets) = data.get("assets").and_then(|a| a.as_array()) {
        for asset in assets {
            let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
            if name.ends_with(".zip") {
                if let Some(url) = asset.get("browser_download_url").and_then(|u| u.as_str()) {
                    return Ok(url.to_string());
                }
            }
        }
    }

    // Fall back to zipball_url (source archive)
    if let Some(url) = data.get("zipball_url").and_then(|u| u.as_str()) {
        return Ok(url.to_string());
    }

    Err("No suitable download URL found in release".to_string())
}

/// Fetch the ZIP download URL from a GitHub release.
async fn fetch_release_download_url(api_url: &str, repo_name: &str) -> Result<String, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(api_url)
        .header("User-Agent", "Claurst/1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("GitHub API request failed: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API returned {}", resp.status()));
    }

    let data: serde_json::Value = resp.json().await
        .map_err(|e| format!("Failed to parse GitHub response: {}", e))?;

    // Find the source archive URL (ZIP)
    let assets = data.get("assets").and_then(|a| a.as_array())
        .ok_or_else(|| "No assets found in release".to_string())?;

    for asset in assets {
        let name = asset.get("name").and_then(|n| n.as_str()).unwrap_or("");
        if name.ends_with(".zip") || name == format!("{}.zip", repo_name) {
            if let Some(browser_download_url) = asset.get("browser_download_url").and_then(|u| u.as_str()) {
                return Ok(browser_download_url.to_string());
            }
        }
    }

    // Fallback: use the source archive URL from the release
    if let Some(source_url) = data.get("zipball_url").and_then(|u| u.as_str()) {
        return Ok(source_url.to_string());
    }

    Err("No suitable download URL found in release".to_string())
}

/// Try to unzip `archive` into `dest`, handling GitHub-style archives that
/// extract to a subdirectory (e.g., `repo-main/`).
///
/// Returns Err if not a valid zip.
fn try_unzip_flat(archive: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    // First, check if the zip extracts to a single subdirectory
    let has_subdir = {
        let first_entry = zip.by_index(0).map_err(|e| e.to_string())?;
        let first_name = first_entry.name().to_string();
        first_name.contains('/') && !first_name.ends_with('/')
    }; // first_entry is dropped here, releasing the borrow on zip

    if has_subdir {
        // GitHub archive style: extracts to `repo-branch/`
        // Extract to a temp dir first, then move contents up
        let temp_dir = dest.join(".tmp_extract");
        std::fs::create_dir_all(&temp_dir).map_err(|e| format!("Create temp dir: {}", e))?;

        zip.extract(&temp_dir).map_err(|e| format!("Extract: {}", e))?;

        // Find the extracted subdirectory
        let entries = std::fs::read_dir(&temp_dir).map_err(|e| e.to_string())?;
        let subdir = entries
            .filter_map(|e| e.ok())
            .find(|e| e.path().is_dir())
            .map(|e| e.path())
            .ok_or_else(|| "No subdirectory found in archive".to_string())?;

        // Move contents from subdirectory to dest
        move_dir_contents(&subdir, dest)?;

        // Clean up temp dir
        std::fs::remove_dir_all(&temp_dir).ok();
    } else {
        // Standard archive: extract directly to dest
        zip.extract(dest).map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Move all contents from `src` directory to `dest` directory.
fn move_dir_contents(src: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    if !src.is_dir() {
        return Err(format!("Source is not a directory: {:?}", src));
    }

    for entry in std::fs::read_dir(src).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());

        if src_path.is_dir() {
            if dest_path.exists() {
                move_dir_contents(&src_path, &dest_path)?;
                std::fs::remove_dir(&src_path).ok();
            } else {
                std::fs::rename(&src_path, &dest_path).map_err(|e| e.to_string())?;
            }
        } else {
            std::fs::rename(&src_path, &dest_path).map_err(|e| e.to_string())?;
        }
    }

    Ok(())
}

/// Check for an update to `name` and download if newer.
pub async fn marketplace_update(name: &str) -> Result<Option<String>, String> {
    let installed = list_installed();
    let current = installed
        .iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("Plugin '{}' is not installed", name))?;

    let results = marketplace_search(name).await?;
    let latest = results
        .iter()
        .find(|e| e.name == name)
        .ok_or_else(|| format!("Plugin '{}' not found in marketplace", name))?;

    if latest.version == current.version {
        return Ok(None); // Already up to date
    }

    marketplace_install(name).await?;
    Ok(Some(latest.version.clone()))
}

/// List all installed plugins.
pub fn list_installed() -> Vec<InstalledPlugin> {
    let plugins_dir = dirs::home_dir()
        .map(|h| h.join(".claurst").join("plugins"))
        .unwrap_or_default();

    let Ok(entries) = std::fs::read_dir(&plugins_dir) else {
        return Vec::new();
    };

    entries
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            if !path.is_dir() {
                return None;
            }
            let name = path.file_name()?.to_string_lossy().to_string();

            let yaml_path = path.join("manifest.yaml");
            let json_path = path.join("manifest.json");

            let (version, description) = if yaml_path.exists() {
                let content = std::fs::read_to_string(&yaml_path).unwrap_or_default();
                let version = extract_yaml_str(&content, "version")
                    .unwrap_or_else(|| "0.0.0".to_string());
                let description = extract_yaml_str(&content, "description").unwrap_or_default();
                (version, description)
            } else if json_path.exists() {
                let content = std::fs::read_to_string(&json_path).unwrap_or_default();
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                    (
                        v["version"].as_str().unwrap_or("0.0.0").to_string(),
                        v["description"].as_str().unwrap_or("").to_string(),
                    )
                } else {
                    ("0.0.0".to_string(), String::new())
                }
            } else {
                ("0.0.0".to_string(), String::new())
            };

            Some(InstalledPlugin {
                name,
                version,
                install_path: path,
                description,
            })
        })
        .collect()
}

/// Uninstall a plugin by removing its directory.
pub fn marketplace_uninstall(name: &str) -> Result<(), String> {
    let dir = plugin_install_dir(name);
    if !dir.exists() {
        return Err(format!("Plugin '{}' is not installed", name));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| format!("Remove dir: {e}"))
}

fn plugin_install_dir(name: &str) -> std::path::PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".claurst")
        .join("plugins")
        .join(name)
}

fn extract_yaml_str(content: &str, key: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix(&format!("{key}:")) {
            return Some(
                rest.trim()
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string(),
            );
        }
    }
    None
}
