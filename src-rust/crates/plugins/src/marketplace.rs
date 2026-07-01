//! Plugin marketplace — GitHub-based marketplace support.
//!
//! Adds support for:
//! - `/plugin marketplace add <source>` — add a marketplace by GitHub shorthand, URL, or local path
//! - `/plugin install plugin@marketplace` — install a specific plugin from a marketplace

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A plugin entry from a marketplace manifest.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginMarketplaceEntry {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub author: Option<String>,
    /// How to fetch this plugin — relative path or full source object.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PluginSourceVariant>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
}

/// A marketplace manifest (marketplace.json).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceManifest {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub owner: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plugins: Vec<PluginMarketplaceEntry>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<MarketplaceMetadata>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force_remove_deleted_plugins: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_cross_marketplace_dependencies_on: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin_root: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// Where a plugin's source comes from.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum PluginSourceVariant {
    /// Relative path within the marketplace repo.
    #[serde(rename = "path")]
    Path(String),
    /// NPM package.
    Npm {
        package: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        version: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        registry: Option<String>,
    },
    /// GitHub repo shorthand.
    Github {
        repo: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        r#ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
    /// Direct git URL.
    Url {
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        r#ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
    /// Git subdirectory clone.
    GitSubdir {
        url: String,
        path: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        r#ref: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
    },
}

impl PluginSourceVariant {
    /// Get a display string for the source.
    pub fn display(&self) -> String {
        match self {
            PluginSourceVariant::Path(p) => p.clone(),
            PluginSourceVariant::Npm { package, version, .. } => {
                if let Some(v) = version {
                    format!("npm:{}@{}", package, v)
                } else {
                    format!("npm:{}", package)
                }
            }
            PluginSourceVariant::Github { repo, r#ref, .. } => {
                if let Some(r) = r#ref {
                    format!("github:{}/{}", repo, r)
                } else {
                    format!("github:{}", repo)
                }
            }
            PluginSourceVariant::Url { url, .. } => url.clone(),
            PluginSourceVariant::GitSubdir { url, path, .. } => format!("{}?path={}", url, path),
        }
    }
}

/// Parsed marketplace input from user.
#[derive(Debug, Clone)]
pub enum MarketplaceSource {
    /// github shorthand: owner/repo or owner/repo@ref
    Github { repo: String, r#ref: Option<String> },
    /// git@host:owner/repo.git URL
    GitSsh { url: String, r#ref: Option<String> },
    /// https://github.com/owner/repo.git
    GitHttps { url: String, r#ref: Option<String> },
    /// Direct URL to marketplace.json
    Url { url: String },
    /// Local file path to marketplace.json
    File { path: PathBuf },
    /// Local directory containing .claude-plugin/marketplace.json
    Directory { path: PathBuf },
}

impl MarketplaceSource {
    /// Parse a marketplace source string into a MarketplaceSource variant.
    pub fn parse(input: &str) -> Option<Self> {
        let trimmed = input.trim();

        // git@ SSH URL: git@github.com:owner/repo.git
        if trimmed.starts_with("git@") && trimmed.contains(':') {
            let (url, reference) = Self::extract_git_ref(trimmed);
            return Some(MarketplaceSource::GitSsh { url: url.to_string(), r#ref: reference });
        }

        // https:// or http:// URL
        if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
            let (url, reference) = Self::extract_url_ref(trimmed);
            if url.contains("github.com") {
                return Some(MarketplaceSource::GitHttps { url: url.to_string(), r#ref: reference });
            }
            return Some(MarketplaceSource::Url { url: url.to_string() });
        }

        // Local file path (starts with ./ or .\)
        if trimmed.starts_with("./") || trimmed.starts_with(".\\") {
            let path = PathBuf::from(trimmed);
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                return Some(MarketplaceSource::File { path });
            }
            return Some(MarketplaceSource::Directory { path });
        }

        // Absolute Windows path
        if trimmed.len() >= 3 && trimmed.chars().nth(1) == Some(':') {
            let path = PathBuf::from(trimmed);
            if path.extension().map(|e| e == "json").unwrap_or(false) {
                return Some(MarketplaceSource::File { path });
            }
            return Some(MarketplaceSource::Directory { path });
        }

        // GitHub shorthand: owner/repo or owner/repo@ref
        if trimmed.contains('/') && !trimmed.starts_with('@') {
            let parts: Vec<&str> = trimmed.splitn(2, '@').collect();
            let repo = parts[0].to_string();
            let reference = parts.get(1).map(|s| s.to_string());
            return Some(MarketplaceSource::Github { repo, r#ref: reference });
        }

        None
    }

    fn extract_git_ref(input: &str) -> (&str, Option<String>) {
        if let Some(hash_pos) = input.find('#') {
            let (url, reference) = input.split_at(hash_pos);
            return (url, Some(reference[1..].to_string()));
        }
        (input, None)
    }

    fn extract_url_ref(input: &str) -> (&str, Option<String>) {
        if let Some(hash_pos) = input.find('#') {
            let (url, reference) = input.split_at(hash_pos);
            return (url, Some(reference[1..].to_string()));
        }
        (input, None)
    }

    /// Get a unique cache key for this source.
    pub fn cache_key(&self) -> String {
        match self {
            MarketplaceSource::Github { repo, .. } => format!("github:{}", repo),
            MarketplaceSource::GitSsh { url, .. } => format!("git:{}", url),
            MarketplaceSource::GitHttps { url, .. } => format!("git:{}", url),
            MarketplaceSource::Url { url } => format!("url:{}", url),
            MarketplaceSource::File { path } => format!("file:{}", path.display()),
            MarketplaceSource::Directory { path } => format!("dir:{}", path.display()),
        }
    }
}

/// An installed plugin summary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub install_path: PathBuf,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// A cached marketplace record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnownMarketplace {
    pub name: String,
    pub source: MarketplaceSourceInfo,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub install_location: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_updated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auto_update: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MarketplaceSourceInfo {
    #[serde(rename = "github")]
    Github { repo: String, r#ref: Option<String> },
    #[serde(rename = "git_ssh")]
    GitSsh { url: String, r#ref: Option<String> },
    #[serde(rename = "git_https")]
    GitHttps { url: String, r#ref: Option<String> },
    Url { url: String },
    File { path: String },
    Directory { path: String },
}

impl MarketplaceSourceInfo {
    pub fn from_source(source: &MarketplaceSource) -> Self {
        match source {
            MarketplaceSource::Github { repo, r#ref } => {
                MarketplaceSourceInfo::Github { repo: repo.clone(), r#ref: r#ref.clone() }
            }
            MarketplaceSource::GitSsh { url, r#ref } => {
                MarketplaceSourceInfo::GitSsh { url: url.clone(), r#ref: r#ref.clone() }
            }
            MarketplaceSource::GitHttps { url, r#ref } => {
                MarketplaceSourceInfo::GitHttps { url: url.clone(), r#ref: r#ref.clone() }
            }
            MarketplaceSource::Url { url } => {
                MarketplaceSourceInfo::Url { url: url.clone() }
            }
            MarketplaceSource::File { path } => {
                MarketplaceSourceInfo::File { path: path.to_string_lossy().into_owned() }
            }
            MarketplaceSource::Directory { path } => {
                MarketplaceSourceInfo::Directory { path: path.to_string_lossy().into_owned() }
            }
        }
    }

    pub fn matches(&self, source: &MarketplaceSource) -> bool {
        match (self, source) {
            (MarketplaceSourceInfo::Github { repo: r1, .. }, MarketplaceSource::Github { repo: r2, .. }) => r1 == r2,
            (MarketplaceSourceInfo::GitSsh { url: u1, .. }, MarketplaceSource::GitSsh { url: u2, .. }) => u1 == u2,
            (MarketplaceSourceInfo::GitHttps { url: u1, .. }, MarketplaceSource::GitHttps { url: u2, .. }) => u1 == u2,
            (MarketplaceSourceInfo::Url { url: u1 }, MarketplaceSource::Url { url: u2 }) => u1 == u2,
            (MarketplaceSourceInfo::File { path: p1 }, MarketplaceSource::File { path: p2 }) => p1 == p2,
            (MarketplaceSourceInfo::Directory { path: p1 }, MarketplaceSource::Directory { path: p2 }) => p1 == p2,
            _ => false,
        }
    }
}

/// Parse a plugin identifier: "plugin@marketplace" or just "plugin".
pub fn parse_plugin_identifier(id: &str) -> (String, Option<String>) {
    if let Some(at_pos) = id.find('@') {
        let name = id[..at_pos].to_string();
        let marketplace = id[at_pos + 1..].to_string();
        (name, Some(marketplace))
    } else {
        (id.to_string(), None)
    }
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

fn get_marketplaces_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_default()
        .join(".claurst")
        .join("plugins")
}

fn get_marketplaces_cache_path() -> PathBuf {
    get_marketplaces_dir().join("known_marketplaces.json")
}

fn get_marketplaces_install_dir() -> PathBuf {
    get_marketplaces_dir().join("marketplaces")
}

fn get_cache_dir() -> PathBuf {
    get_marketplaces_dir().join("cache")
}

// ---------------------------------------------------------------------------
// Marketplace persistence
// ---------------------------------------------------------------------------

/// Load known marketplaces from disk.
pub fn load_known_marketplaces() -> HashMap<String, KnownMarketplace> {
    let path = get_marketplaces_cache_path();
    if !path.exists() {
        return HashMap::new();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
        Err(_) => HashMap::new(),
    }
}

/// Save known marketplaces to disk.
pub fn save_known_marketplaces(marketplaces: &HashMap<String, KnownMarketplace>) -> Result<(), MarketplaceError> {
    let path = get_marketplaces_cache_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| MarketplaceError::Io(e.to_string()))?;
    }
    let content = serde_json::to_string_pretty(marketplaces).map_err(|e| MarketplaceError::Serialize(e.to_string()))?;
    std::fs::write(&path, content).map_err(|e| MarketplaceError::Io(e.to_string()))
}

// ---------------------------------------------------------------------------
// Marketplace operations
// ---------------------------------------------------------------------------

/// Options for adding a marketplace.
#[derive(Debug, Default)]
pub struct MarketplaceAddOptions {
    pub sparse: Option<Vec<String>>,
    pub scope: Option<String>,
}

/// Add a marketplace by source string.
/// Returns the marketplace name and whether it was already cached.
pub async fn add_marketplace_source(
    source_str: &str,
    _options: MarketplaceAddOptions,
) -> Result<(String, bool), MarketplaceError> {
    let source = MarketplaceSource::parse(source_str)
        .ok_or_else(|| MarketplaceError::InvalidSource(source_str.to_string()))?;

    let cache_key = source.cache_key();
    let mut marketplaces = load_known_marketplaces();

    // Idempotency check
    for (name, known) in &marketplaces {
        if known.source.matches(&source) {
            tracing::debug!("Marketplace '{}' already cached for source '{}'", name, cache_key);
            return Ok((name.clone(), true));
        }
    }

    // Load and cache the marketplace
    let (name, install_location) = load_and_cache_marketplace(&source).await?;

    let marketplace = KnownMarketplace {
        name: name.clone(),
        source: MarketplaceSourceInfo::from_source(&source),
        install_location: Some(install_location),
        last_updated: Some(chrono::Utc::now().to_rfc3339()),
        auto_update: Some(false),
    };

    marketplaces.insert(name.clone(), marketplace);
    save_known_marketplaces(&marketplaces)?;

    Ok((name, false))
}

/// Load a marketplace manifest from a source and cache it locally.
async fn load_and_cache_marketplace(source: &MarketplaceSource) -> Result<(String, PathBuf), MarketplaceError> {
    let install_dir = get_marketplaces_install_dir();
    std::fs::create_dir_all(&install_dir).map_err(|e| MarketplaceError::Io(e.to_string()))?;

    match source {
        MarketplaceSource::Github { repo, r#ref } => {
            load_github_marketplace(repo, r#ref.as_deref(), &install_dir).await
        }
        MarketplaceSource::GitHttps { url, r#ref } => {
            load_git_marketplace(url, r#ref.as_deref(), &install_dir).await
        }
        MarketplaceSource::GitSsh { url, r#ref } => {
            load_git_marketplace(url, r#ref.as_deref(), &install_dir).await
        }
        MarketplaceSource::Url { url } => {
            load_url_marketplace(url, &install_dir).await
        }
        MarketplaceSource::File { path } => {
            load_file_marketplace(path, &install_dir)
        }
        MarketplaceSource::Directory { path } => {
            load_directory_marketplace(path, &install_dir)
        }
    }
}

async fn load_github_marketplace(
    repo: &str,
    r#ref: Option<&str>,
    install_dir: &PathBuf,
) -> Result<(String, PathBuf), MarketplaceError> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(MarketplaceError::InvalidSource(format!("Invalid GitHub repo: {}", repo)));
    }
    let (owner, repo_name) = (parts[0], parts[1]);

    let default_branch = get_github_default_branch(owner, repo_name).await?;
    let branch = r#ref.unwrap_or(&default_branch);

    let temp_dir = install_dir.join(format!(".tmp.{}", repo_name.replace('/', "-")));
    let git_url = format!("https://github.com/{}/{}", owner, repo_name);

    clone_git_repo(&git_url, &temp_dir, Some(branch)).await?;

    let manifest = find_marketplace_manifest(&temp_dir)?;
    let marketplace: MarketplaceManifest = serde_json::from_str(&manifest)
        .map_err(|e| MarketplaceError::Parse(e.to_string()))?;

    let name = marketplace.name.clone();
    let target_dir = install_dir.join(&name);

    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).ok();
    }
    std::fs::rename(&temp_dir, &target_dir)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;

    Ok((name, target_dir))
}

async fn load_git_marketplace(
    url: &str,
    r#ref: Option<&str>,
    install_dir: &PathBuf,
) -> Result<(String, PathBuf), MarketplaceError> {
    let repo_name = url
        .split('/')
        .last()
        .unwrap_or("marketplace")
        .trim_end_matches(".git");

    let temp_dir = install_dir.join(format!(".tmp.{}", repo_name));
    clone_git_repo(url, &temp_dir, r#ref).await?;

    let manifest = find_marketplace_manifest(&temp_dir)?;
    let marketplace: MarketplaceManifest = serde_json::from_str(&manifest)
        .map_err(|e| MarketplaceError::Parse(e.to_string()))?;

    let name = marketplace.name.clone();
    let target_dir = install_dir.join(&name);

    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).ok();
    }
    std::fs::rename(&temp_dir, &target_dir)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;

    Ok((name, target_dir))
}

async fn load_url_marketplace(
    url: &str,
    install_dir: &PathBuf,
) -> Result<(String, PathBuf), MarketplaceError> {
    let response = reqwest::get(url).await
        .map_err(|e| MarketplaceError::Network(e.to_string()))?;

    if !response.status().is_success() {
        return Err(MarketplaceError::Network(format!("HTTP {}", response.status())));
    }

    let body = response.text().await
        .map_err(|e| MarketplaceError::Network(e.to_string()))?;

    let marketplace: MarketplaceManifest = serde_json::from_str(&body)
        .map_err(|e| MarketplaceError::Parse(e.to_string()))?;

    let name = marketplace.name.clone();
    let target_dir = install_dir.join(&name);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;

    let manifest_path = target_dir.join("marketplace.json");
    std::fs::write(&manifest_path, &body)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;

    Ok((name, target_dir))
}

fn load_file_marketplace(
    path: &PathBuf,
    install_dir: &PathBuf,
) -> Result<(String, PathBuf), MarketplaceError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;

    let marketplace: MarketplaceManifest = serde_json::from_str(&content)
        .map_err(|e| MarketplaceError::Parse(e.to_string()))?;

    let name = marketplace.name.clone();
    let target_dir = install_dir.join(&name);
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;

    let manifest_path = target_dir.join("marketplace.json");
    std::fs::write(&manifest_path, &content)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;

    Ok((name, target_dir))
}

fn load_directory_marketplace(
    path: &PathBuf,
    install_dir: &PathBuf,
) -> Result<(String, PathBuf), MarketplaceError> {
    let manifest_path = path.join(".claude-plugin").join("marketplace.json");
    let fallback_path = path.join("marketplace.json");
    let manifest_path = if manifest_path.exists() {
        manifest_path
    } else if fallback_path.exists() {
        fallback_path
    } else {
        return Err(MarketplaceError::Parse(format!(
            "No marketplace.json found in {}",
            path.display()
        )));
    };

    let content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;

    let marketplace: MarketplaceManifest = serde_json::from_str(&content)
        .map_err(|e| MarketplaceError::Parse(e.to_string()))?;

    let name = marketplace.name.clone();
    let target_dir = install_dir.join(&name);
    if target_dir.exists() {
        std::fs::remove_dir_all(&target_dir).ok();
    }

    copy_dir_recursive(path, &target_dir)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;
    Ok((name, target_dir))
}

fn copy_dir_recursive(src: &PathBuf, dst: &PathBuf) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        if ty.is_dir() {
            copy_dir_recursive(&entry.path(), &dst.join(entry.file_name()))?;
        } else {
            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
        }
    }
    Ok(())
}

/// Find marketplace.json in a cloned repo.
fn find_marketplace_manifest(dir: &PathBuf) -> Result<String, MarketplaceError> {
    let paths = [
        dir.join(".claude-plugin").join("marketplace.json"),
        dir.join("marketplace.json"),
    ];

    for path in &paths {
        if path.exists() {
            return std::fs::read_to_string(path)
                .map_err(|e| MarketplaceError::Io(e.to_string()));
        }
    }

    Err(MarketplaceError::Parse(format!(
        "No marketplace.json found in {}",
        dir.display()
    )))
}

/// Get the default branch of a GitHub repo.
async fn get_github_default_branch(owner: &str, repo: &str) -> Result<String, MarketplaceError> {
    let client = reqwest::Client::new();
    let url = format!("https://api.github.com/repos/{}/{}", owner, repo);

    let resp = client
        .get(&url)
        .header("User-Agent", "Claurst/1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| MarketplaceError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(MarketplaceError::Network(format!("GitHub API: {}", resp.status())));
    }

    let data: serde_json::Value = resp.json().await
        .map_err(|e| MarketplaceError::Parse(e.to_string()))?;

    let branch = data.get("default_branch")
        .and_then(|b| b.as_str())
        .unwrap_or("main")
        .to_string();

    Ok(branch)
}

/// Clone a git repository shallowly to a target directory.
async fn clone_git_repo(url: &str, target: &PathBuf, branch: Option<&str>) -> Result<(), MarketplaceError> {
    use tokio::process::Command;

    std::fs::create_dir_all(target).map_err(|e| MarketplaceError::Io(e.to_string()))?;

    let mut cmd = Command::new("git");
    cmd.args(["clone", "--depth=1", "--recurse-submodules"]);

    if let Some(b) = branch {
        cmd.arg("--branch").arg(b);
    }

    cmd.arg(url).arg(target.to_str().unwrap_or("."));

    cmd.output().await
        .map_err(|e| MarketplaceError::Git(e.to_string()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Plugin install from marketplace
// ---------------------------------------------------------------------------

/// Install a plugin by identifier: "plugin@marketplace" or just "plugin".
pub async fn install_plugin_from_marketplace(
    plugin_id: &str,
    _scope: Option<&str>,
) -> Result<InstalledPlugin, MarketplaceError> {
    let (plugin_name, marketplace_name) = parse_plugin_identifier(plugin_id);

    if let Some(marketplace) = marketplace_name {
        install_from_specific_marketplace(&plugin_name, &marketplace).await
    } else {
        let marketplaces = load_known_marketplaces();
        for (_name, known) in &marketplaces {
            if let Some(loc) = &known.install_location {
                if let Ok(plugin) = install_plugin_from_dir(&plugin_name, loc).await {
                    return Ok(plugin);
                }
            }
        }
        Err(MarketplaceError::PluginNotFound(plugin_name))
    }
}

async fn install_from_specific_marketplace(
    plugin_name: &str,
    marketplace_name: &str,
) -> Result<InstalledPlugin, MarketplaceError> {
    let marketplaces = load_known_marketplaces();

    let known = marketplaces.get(marketplace_name)
        .ok_or_else(|| MarketplaceError::MarketplaceNotFound(marketplace_name.to_string()))?;

    let install_location = known.install_location.as_ref()
        .ok_or_else(|| MarketplaceError::MarketplaceNotFound(marketplace_name.to_string()))?;

    install_plugin_from_dir(plugin_name, install_location).await
}

async fn install_plugin_from_dir(
    plugin_name: &str,
    marketplace_dir: &PathBuf,
) -> Result<InstalledPlugin, MarketplaceError> {
    let manifest_path = marketplace_dir.join("marketplace.json");
    if !manifest_path.exists() {
        return Err(MarketplaceError::Parse(format!(
            "marketplace.json not found in {}",
            marketplace_dir.display()
        )));
    }

    let content = std::fs::read_to_string(&manifest_path)
        .map_err(|e| MarketplaceError::Io(e.to_string()))?;

    let marketplace: MarketplaceManifest = serde_json::from_str(&content)
        .map_err(|e| MarketplaceError::Parse(e.to_string()))?;

    let entry = marketplace.plugins.iter()
        .find(|p| p.name == plugin_name)
        .ok_or_else(|| MarketplaceError::PluginNotFound(plugin_name.to_string()))?;

    let plugin_dir = get_plugin_cache_dir(&marketplace.name, plugin_name);
    download_plugin_source(entry, marketplace_dir, &plugin_dir).await?;

    Ok(InstalledPlugin {
        name: plugin_name.to_string(),
        version: entry.version.clone(),
        install_path: plugin_dir,
        description: entry.description.clone(),
    })
}

fn get_plugin_cache_dir(marketplace: &str, plugin: &str) -> PathBuf {
    get_cache_dir().join(marketplace).join(plugin)
}

async fn download_plugin_source(
    entry: &PluginMarketplaceEntry,
    marketplace_dir: &PathBuf,
    target_dir: &PathBuf,
) -> Result<(), MarketplaceError> {
    use tokio::process::Command;

    std::fs::create_dir_all(target_dir).map_err(|e| MarketplaceError::Io(e.to_string()))?;

    match entry.source.as_ref() {
        Some(PluginSourceVariant::Path(path)) => {
            let src = marketplace_dir.join(path);
            if src.is_dir() {
                copy_dir_recursive(&src, target_dir).map_err(|e| MarketplaceError::Io(e.to_string()))?;
            } else {
                std::fs::copy(&src, target_dir.join("plugin.json"))
                    .map_err(|e| MarketplaceError::Io(e.to_string()))?;
            }
        }
        Some(PluginSourceVariant::Github { repo, r#ref, .. }) => {
            let git_url = format!("https://github.com/{}", repo);
            clone_git_repo(&git_url, target_dir, r#ref.as_deref()).await?;
        }
        Some(PluginSourceVariant::Url { url, r#ref, .. }) => {
            clone_git_repo(url, target_dir, r#ref.as_deref()).await?;
        }
        Some(PluginSourceVariant::GitSubdir { url, path, r#ref, .. }) => {
            let temp_dir = target_dir.join(".tmp");
            clone_git_repo(url, &temp_dir, r#ref.as_deref()).await?;
            let subdir = temp_dir.join(path);
            if subdir.exists() {
                copy_dir_recursive(&subdir, target_dir).map_err(|e| MarketplaceError::Io(e.to_string()))?;
                std::fs::remove_dir_all(&temp_dir).ok();
            }
        }
        Some(PluginSourceVariant::Npm { package, version, .. }) => {
            let pkg = if let Some(v) = version {
                format!("{}@{}", package, v)
            } else {
                package.to_string()
            };
            Command::new("npm")
                .args(["install", &pkg])
                .current_dir(target_dir)
                .output().await
                .map_err(|e| MarketplaceError::Npm(e.to_string()))?;
        }
        None => {
            let src = marketplace_dir.join(&entry.name);
            if src.is_dir() {
                copy_dir_recursive(&src, target_dir).map_err(|e| MarketplaceError::Io(e.to_string()))?;
            } else {
                return Err(MarketplaceError::Parse(format!(
                    "Plugin '{}' has no source and no directory found in marketplace",
                    entry.name
                )));
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// List / search / remove
// ---------------------------------------------------------------------------

/// List all known/cached marketplaces.
pub fn list_marketplaces() -> Vec<KnownMarketplace> {
    load_known_marketplaces().into_values().collect()
}

/// Search plugins across all cached marketplaces.
pub async fn search_marketplaces(query: &str) -> Vec<(String, PluginMarketplaceEntry)> {
    let marketplaces = load_known_marketplaces();
    let query_lower = query.to_lowercase();
    let mut results = Vec::new();

    for (mkt_name, known) in &marketplaces {
        if let Some(loc) = &known.install_location {
            let manifest_path = loc.join("marketplace.json");
            if manifest_path.exists() {
                if let Ok(content) = std::fs::read_to_string(&manifest_path) {
                    if let Ok(marketplace) = serde_json::from_str::<MarketplaceManifest>(&content) {
                        for plugin in marketplace.plugins {
                            if plugin.name.to_lowercase().contains(&query_lower)
                                || plugin.description.as_ref().map(|d| d.to_lowercase().contains(&query_lower)).unwrap_or(false)
                            {
                                results.push((mkt_name.clone(), plugin));
                            }
                        }
                    }
                }
            }
        }
    }

    results
}

/// Remove a cached marketplace.
pub fn remove_marketplace(name: &str) -> Result<(), MarketplaceError> {
    let mut marketplaces = load_known_marketplaces();

    let removed = marketplaces.remove(name)
        .ok_or_else(|| MarketplaceError::MarketplaceNotFound(name.to_string()))?;

    save_known_marketplaces(&marketplaces)?;

    if let Some(loc) = removed.install_location {
        std::fs::remove_dir_all(&loc).ok();
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum MarketplaceError {
    #[error("Invalid marketplace source: {0}")]
    InvalidSource(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error("Git error: {0}")]
    Git(String),

    #[error("NPM error: {0}")]
    Npm(String),

    #[error("IO error: {0}")]
    Io(String),

    #[error("Serialize error: {0}")]
    Serialize(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Marketplace not found: {0}")]
    MarketplaceNotFound(String),

    #[error("Plugin not found: {0}")]
    PluginNotFound(String),
}

// ---------------------------------------------------------------------------
// Legacy API compatibility
// ---------------------------------------------------------------------------

const REGISTRY_URL: &str = "https://registry.claude.ai/plugins";

/// A plugin entry from the old registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryPluginEntry {
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub download_url: String,
    pub hash: String,
    pub tags: Vec<String>,
    pub updated_at: Option<u64>,
}

/// Search the old registry (kept for compatibility).
pub async fn marketplace_search(query: &str) -> Result<Vec<InstalledPlugin>, String> {
    let resp = reqwest::get(REGISTRY_URL).await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        return Err(format!("Registry returned {}", resp.status()));
    }

    let entries: Vec<RegistryPluginEntry> = resp.json().await
        .map_err(|e| e.to_string())?;

    let query_lower = query.to_lowercase();
    let results: Vec<InstalledPlugin> = entries
        .into_iter()
        .filter(|e| e.name.to_lowercase().contains(&query_lower))
        .map(|e| InstalledPlugin {
            name: e.name,
            version: Some(e.version),
            install_path: PathBuf::from(e.download_url),
            description: Some(e.description),
        })
        .collect();

    Ok(results)
}

/// Install from the old registry (kept for compatibility).
pub async fn marketplace_install(name_or_url: &str) -> Result<InstalledPlugin, String> {
    if name_or_url.starts_with("http") {
        let name = name_or_url.split('/').last()
            .unwrap_or("plugin")
            .trim_end_matches(".zip")
            .to_string();

        let install_dir = get_cache_dir().join(&name);
        std::fs::create_dir_all(&install_dir).map_err(|e| e.to_string())?;

        let response = reqwest::get(name_or_url).await
            .map_err(|e| e.to_string())?;
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;

        let archive_path = install_dir.join("plugin.zip");
        std::fs::write(&archive_path, &bytes).map_err(|e| e.to_string())?;

        if try_unzip_flat(&archive_path, &install_dir).is_err() {
            let manifest_path = install_dir.join("manifest.json");
            std::fs::copy(&archive_path, &manifest_path).map_err(|e| e.to_string())?;
            std::fs::remove_file(&archive_path).ok();
        }

        return Ok(InstalledPlugin {
            name,
            version: Some("0.0.0".to_string()),
            install_path: install_dir,
            description: None,
        });
    }

    if name_or_url.contains('/') {
        let parts: Vec<&str> = name_or_url.splitn(2, '@').collect();
        let repo = parts[0].to_string();
        let tag = parts.get(1).map(|s| s.to_string());

        let (entry, install_dir) = install_from_github_internal(&repo, tag.as_deref()).await?;

        let response = reqwest::get(&entry.download_url).await
            .map_err(|e| e.to_string())?;
        let bytes = response.bytes().await.map_err(|e| e.to_string())?;

        let archive_path = install_dir.join("plugin.zip");
        std::fs::write(&archive_path, &bytes).map_err(|e| e.to_string())?;

        try_unzip_flat(&archive_path, &install_dir).ok();
        std::fs::remove_file(&archive_path).ok();

        return Ok(InstalledPlugin {
            name: entry.name,
            version: Some(entry.version),
            install_path: install_dir,
            description: Some(entry.description),
        });
    }

    let results = marketplace_search(name_or_url).await?;
    results.into_iter()
        .find(|p| p.name == name_or_url)
        .ok_or_else(|| format!("Plugin '{}' not found in marketplace", name_or_url))
}

/// Install a plugin directly from a GitHub repository.
pub async fn install_from_github(
    repo: &str,
    tag: Option<&str>,
) -> Result<(RegistryPluginEntry, PathBuf), String> {
    install_from_github_internal(repo, tag).await
}

async fn install_from_github_internal(
    repo: &str,
    tag: Option<&str>,
) -> Result<(RegistryPluginEntry, PathBuf), String> {
    let parts: Vec<&str> = repo.split('/').collect();
    if parts.len() != 2 {
        return Err(format!("Invalid GitHub repo format: '{}'", repo));
    }
    let (owner, repo_name) = (parts[0], parts[1]);

    let default_branch = get_github_default_branch_internal(owner, repo_name).await
        .map_err(|e| e.to_string())?;

    let download_url = if let Some(tag) = tag {
        format!(
            "https://github.com/{}/{}/archive/refs/heads/{}.zip",
            owner, repo_name, tag
        )
    } else {
        format!(
            "https://github.com/{}/{}/archive/refs/heads/{}.zip",
            owner, repo_name, default_branch
        )
    };

    let install_dir = get_cache_dir().join(repo_name);

    Ok((
        RegistryPluginEntry {
            name: repo_name.to_string(),
            version: tag.unwrap_or(&default_branch).to_string(),
            description: format!("Installed from {}/{}", owner, repo_name),
            author: owner.to_string(),
            download_url,
            hash: String::new(),
            tags: vec!["github".to_string()],
            updated_at: None,
        },
        install_dir,
    ))
}

async fn get_github_default_branch_internal(owner: &str, repo: &str) -> Result<String, MarketplaceError> {
    let client = reqwest::Client::new();
    let url = format!("https://api.github.com/repos/{}/{}", owner, repo);

    let resp = client
        .get(&url)
        .header("User-Agent", "Claurst/1.0")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| MarketplaceError::Network(e.to_string()))?;

    if !resp.status().is_success() {
        return Err(MarketplaceError::Network(format!("GitHub API: {}", resp.status())));
    }

    let data: serde_json::Value = resp.json().await
        .map_err(|e| MarketplaceError::Parse(e.to_string()))?;

    let branch = data.get("default_branch")
        .and_then(|b| b.as_str())
        .unwrap_or("main")
        .to_string();

    Ok(branch)
}

/// Try to unzip an archive into a flat directory.
fn try_unzip_flat(archive: &PathBuf, dest: &PathBuf) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;

    let has_subdir = {
        let first_entry = zip.by_index(0).map_err(|e| e.to_string())?;
        let first_name = first_entry.name().to_string();
        first_name.contains('/') && !first_name.ends_with('/')
    };

    if has_subdir {
        let temp_dir = dest.join(".tmp_extract");
        std::fs::create_dir_all(&temp_dir).map_err(|e| e.to_string())?;
        zip.extract(&temp_dir).map_err(|e| e.to_string())?;

        let entries = std::fs::read_dir(&temp_dir).map_err(|e| e.to_string())?;
        let subdir = entries
            .filter_map(|e| e.ok())
            .find(|e| e.path().is_dir())
            .map(|e| e.path())
            .ok_or_else(|| "No subdirectory found in archive".to_string())?;

        move_dir_contents(&subdir, dest).map_err(|e| e.to_string())?;
        std::fs::remove_dir_all(&temp_dir).ok();
    } else {
        zip.extract(dest).map_err(|e| e.to_string())?;
    }

    Ok(())
}

/// Move all contents from src to dest.
fn move_dir_contents(src: &PathBuf, dest: &PathBuf) -> std::io::Result<()> {
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dest_path = dest.join(entry.file_name());
        if src_path.is_dir() {
            if dest_path.exists() {
                move_dir_contents(&src_path, &dest_path)?;
                std::fs::remove_dir(&src_path)?;
            } else {
                std::fs::rename(&src_path, &dest_path)?;
            }
        } else {
            std::fs::rename(&src_path, &dest_path)?;
        }
    }
    Ok(())
}

/// Check for updates.
pub async fn marketplace_check_updates_all() -> Vec<(String, String, String)> {
    let installed = list_installed();
    let mut futures_vec = Vec::new();

    for plugin in &installed {
        let name = plugin.name.clone();
        let current = plugin.version.clone().unwrap_or_default();
        futures_vec.push(async move {
            if let Ok(results) = marketplace_search(&name).await {
                if let Some(latest) = results.iter().find(|e| e.name == name) {
                    if latest.version.as_ref() != Some(&current) {
                        return Some((name, current, latest.version.clone().unwrap_or_default()));
                    }
                }
            }
            None
        });
    }

    futures::future::join_all(futures_vec).await
        .into_iter()
        .flatten()
        .collect()
}

/// List all locally installed plugins.
pub fn list_installed() -> Vec<InstalledPlugin> {
    let plugins_dir = get_cache_dir();
    if !plugins_dir.exists() {
        return Vec::new();
    }

    std::fs::read_dir(&plugins_dir)
        .map(|entries| {
            entries.flatten()
                .filter(|e| e.path().is_dir())
                .filter_map(|e| {
                    let path = e.path();
                    let name = path.file_name()?.to_string_lossy().to_string();

                    let json_path = path.join("manifest.json");
                    let json_path2 = path.join("plugin.json");

                    let (version, description) = if json_path.exists() {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&json_path).unwrap_or_default()) {
                            (v["version"].as_str().map(|s| s.to_string()), v["description"].as_str().map(|s| s.to_string()))
                        } else {
                            (None, None)
                        }
                    } else if json_path2.exists() {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&std::fs::read_to_string(&json_path2).unwrap_or_default()) {
                            (v["version"].as_str().map(|s| s.to_string()), v["description"].as_str().map(|s| s.to_string()))
                        } else {
                            (None, None)
                        }
                    } else {
                        (None, None)
                    };

                    Some(InstalledPlugin {
                        name,
                        version,
                        install_path: path,
                        description,
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Uninstall a plugin.
pub fn marketplace_uninstall(name: &str) -> Result<(), String> {
    let plugin_dir = get_cache_dir().join(name);
    if !plugin_dir.exists() {
        return Err(format!("Plugin '{}' is not installed", name));
    }
    std::fs::remove_dir_all(&plugin_dir).map_err(|e| e.to_string())
}

/// Update a plugin.
pub async fn marketplace_update(name: &str) -> Result<Option<String>, String> {
    let installed = list_installed();
    let current = installed.iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("Plugin '{}' is not installed", name))?;

    let current_ver = current.version.clone().unwrap_or_default();
    let results = marketplace_search(name).await?;
    let latest = results.iter()
        .find(|p| p.name == name)
        .ok_or_else(|| format!("Plugin '{}' not found in marketplace", name))?;

    let latest_ver = latest.version.clone().unwrap_or_default();
    if latest_ver == current_ver {
        return Ok(None);
    }

    marketplace_install(name).await?;
    Ok(Some(latest_ver))
}
