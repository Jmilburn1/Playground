use crate::sources::{ModSearchResult, ModSource};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};

/// GitHub update source — the first real source (Phase 4).
///
/// A mod is linked to a repo ("owner/repo", stored in the config
/// keyed by the mod's file stem); the check reads the repo's
/// latest release through the GitHub API and compares its tag
/// against the mod's version. Installing downloads the release's
/// mod .zip asset, which the manager then swaps over the old
/// file. The API base comes from the config's GitHub source entry
/// (default https://api.github.com), so tests can point it at a
/// local mock server.
pub struct GithubSource {
    token: Option<String>,
}

impl GithubSource {
    pub fn new(token: Option<String>) -> Self {
        Self { token }
    }
}

/// One release asset from the GitHub API.
#[derive(Debug, Clone, Deserialize)]
pub struct AssetInfo {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
}

/// The parts of a GitHub release Playground uses.
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    /// The release tag as published ("v1.2.0").
    pub tag: String,
    /// The tag normalized for comparison ("1.2.0").
    pub version: String,
    pub assets: Vec<AssetInfo>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<AssetInfo>,
}

fn client() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .user_agent("Playground-Mod-Manager")
        .build()?)
}

fn auth(req: reqwest::RequestBuilder, token: &Option<String>) -> reqwest::RequestBuilder {
    match token {
        Some(t) if !t.trim().is_empty() => req.bearer_auth(t.trim()),
        _ => req,
    }
}

/// Normalize a user-typed repo reference to "owner/repo".
/// Accepts "owner/repo", a full github.com URL, a trailing
/// slash, and a ".git" suffix.
pub fn normalize_repo(input: &str) -> Option<String> {
    let mut s = input.trim().to_string();
    if let Some(rest) = s
        .strip_prefix("https://github.com/")
        .or_else(|| s.strip_prefix("http://github.com/"))
        .or_else(|| s.strip_prefix("github.com/"))
    {
        s = rest.to_string();
    }
    let s = s.trim_end_matches('/').trim_end_matches(".git");
    let parts: Vec<&str> = s.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() == 2 {
        Some(format!("{}/{}", parts[0], parts[1]))
    } else {
        None
    }
}

/// Parse a version string into numeric parts: strips a leading
/// "v" (or any non-numeric prefix) and splits on dots. "1.0.0.2"
/// -> [1,0,0,2]; "v1.2" -> [1,2]. Non-numeric segments stop the
/// parse (release tags like "1.0-beta" still yield [1,0]).
pub fn parse_version(s: &str) -> Vec<u64> {
    let start = s
        .find(|c: char| c.is_ascii_digit())
        .unwrap_or(usize::MAX);
    if start == usize::MAX {
        return Vec::new();
    }
    let mut parts = Vec::new();
    for seg in s[start..].split('.') {
        let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
        if digits.is_empty() {
            break;
        }
        match digits.parse::<u64>() {
            Ok(n) => parts.push(n),
            Err(_) => break,
        }
    }
    parts
}

/// True when `candidate` is a strictly newer version than
/// `current`. Both are compared as dotted numbers, missing parts
/// counting as 0 ("1.2" vs "1.2.0.0" are equal). A candidate with
/// no parseable numbers is never "newer" — a weird tag must not
/// trigger an overwrite on a guess.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let c = parse_version(candidate);
    if c.is_empty() {
        return false;
    }
    let cur = parse_version(current);
    let len = c.len().max(cur.len());
    for i in 0..len {
        let a = c.get(i).copied().unwrap_or(0);
        let b = cur.get(i).copied().unwrap_or(0);
        if a != b {
            return a > b;
        }
    }
    false
}

/// Pick the release asset that is the mod file for `stem`
/// (lowercase file stem, no .zip): an exact filename match wins,
/// then a .zip whose stem starts with the mod stem, then the
/// only .zip asset, then the largest .zip. Assets that are not
/// .zip files never qualify — a release full of installers and
/// notes has no mod file to install.
pub fn pick_asset<'a>(release: &'a ReleaseInfo, stem: &str) -> Option<&'a AssetInfo> {
    let zips: Vec<&AssetInfo> = release
        .assets
        .iter()
        .filter(|a| a.name.to_lowercase().ends_with(".zip"))
        .collect();
    let asset_stem = |a: &AssetInfo| {
        a.name
            .to_lowercase()
            .strip_suffix(".zip")
            .unwrap_or("")
            .to_string()
    };
    if let Some(a) = zips.iter().find(|a| asset_stem(a) == stem) {
        return Some(a);
    }
    if let Some(a) = zips.iter().find(|a| asset_stem(a).starts_with(stem)) {
        return Some(a);
    }
    if zips.len() == 1 {
        return Some(zips[0]);
    }
    zips.into_iter().max_by_key(|a| a.size)
}

impl GithubSource {
    /// The repo's latest published release, or None when the
    /// repo has no releases (GitHub answers 404 for that).
    pub async fn latest_release(
        &self,
        base: &str,
        repo: &str,
    ) -> Result<Option<ReleaseInfo>> {
        let url = format!("{}/repos/{}/releases/latest", base.trim_end_matches('/'), repo);
        let resp = auth(client()?.get(&url), &self.token)
            .header("Accept", "application/vnd.github+json")
            .send()
            .await
            .with_context(|| format!("Could not reach GitHub for {}", repo))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let resp = resp
            .error_for_status()
            .with_context(|| format!("GitHub release check failed for {}", repo))?;
        let api: ApiRelease = resp
            .json()
            .await
            .context("GitHub sent an unreadable release reply")?;
        Ok(Some(ReleaseInfo {
            version: parse_version(&api.tag_name)
                .iter()
                .map(|n| n.to_string())
                .collect::<Vec<_>>()
                .join("."),
            tag: api.tag_name,
            assets: api.assets,
        }))
    }

    /// Stream a release asset to `dest` (a temp file the caller
    /// validates before swapping in). Returns the byte count.
    pub async fn download_to(&self, url: &str, dest: &Path) -> Result<u64> {
        use futures::StreamExt;
        let resp = auth(client()?.get(url), &self.token)
            .header("Accept", "application/octet-stream")
            .send()
            .await
            .context("Download did not start")?
            .error_for_status()
            .context("Download was refused")?;
        let mut file = tokio::fs::File::create(dest)
            .await
            .with_context(|| format!("Could not create {:?}", dest))?;
        let mut stream = resp.bytes_stream();
        let mut written: u64 = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Download interrupted")?;
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                .await
                .context("Could not write the downloaded file")?;
            written += chunk.len() as u64;
        }
        tokio::io::AsyncWriteExt::flush(&mut file)
            .await
            .context("Could not finish the downloaded file")?;
        Ok(written)
    }
}

// The original synchronous trait stays for the other sources;
// GitHub's real work goes through the async methods above (the
// GUI runs them as background tasks, the CLI on a runtime).
impl ModSource for GithubSource {
    fn name(&self) -> &str {
        "GitHub"
    }

    fn search(&self, _query: &str, _game_version: &str) -> Result<Vec<ModSearchResult>> {
        Ok(Vec::new())
    }

    fn download(&self, _mod_id: &str, _dest: PathBuf) -> Result<PathBuf> {
        Err(anyhow::anyhow!("Use the update install flow"))
    }

    fn get_mod_info(&self, _mod_id: &str) -> Result<ModSearchResult> {
        Err(anyhow::anyhow!("Use the update check flow"))
    }

    fn check_updates(
        &self,
        _mod_id: &str,
        _current_version: &str,
    ) -> Result<Option<ModSearchResult>> {
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_dotted_numbers() {
        assert!(is_newer("1.0.0.2", "1.0.0.1"));
        assert!(is_newer("v1.1", "1.0.9.9"));
        assert!(is_newer("2", "1.9.9.9"));
        assert!(!is_newer("1.0.0.0", "1.0.0.0"));
        assert!(!is_newer("1.2", "1.2.0.0"));
        assert!(!is_newer("1.0.0.1", "1.0.0.2"));
        assert!(!is_newer("beta", "1.0.0.0"));
        assert!(!is_newer("", "1.0.0.0"));
    }

    #[test]
    fn repo_references_normalize() {
        assert_eq!(normalize_repo("owner/repo").as_deref(), Some("owner/repo"));
        assert_eq!(
            normalize_repo("https://github.com/owner/repo").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(
            normalize_repo("https://github.com/owner/repo.git").as_deref(),
            Some("owner/repo")
        );
        assert_eq!(normalize_repo("just-a-name"), None);
        assert_eq!(normalize_repo("a/b/c"), None);
    }

    fn release(assets: Vec<(&str, u64)>) -> ReleaseInfo {
        ReleaseInfo {
            tag: "v1".to_string(),
            version: "1".to_string(),
            assets: assets
                .into_iter()
                .map(|(name, size)| AssetInfo {
                    name: name.to_string(),
                    browser_download_url: "https://example.invalid/x".to_string(),
                    size,
                })
                .collect(),
        }
    }

    #[test]
    fn asset_picking_prefers_the_mod_zip() {
        let rel = release(vec![
            ("notes.zip", 10),
            ("FS25_Thing.zip", 100),
            ("FS25_Thing_manual.pdf", 5),
        ]);
        assert_eq!(pick_asset(&rel, "fs25_thing").unwrap().name, "FS25_Thing.zip");

        // One zip among other files: take the zip.
        let rel = release(vec![("setup.exe", 999), ("modpack.zip", 50)]);
        assert_eq!(pick_asset(&rel, "fs25_other").unwrap().name, "modpack.zip");

        // No zip at all: nothing to install.
        let rel = release(vec![("setup.exe", 999)]);
        assert!(pick_asset(&rel, "fs25_other").is_none());
    }
}
