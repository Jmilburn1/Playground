//! The client side of the Playground catalog: one static
//! JSON index of ModHub (filename stem -> mod_id, current
//! version, download URL), published by the indexer
//! (`cli catalog-build`) and fetched from a plain URL.
//!
//! Why: without the catalog, every user's app has to search
//! ModHub mod by mod to detect links or check versions —
//! slow for the user and noisy for ModHub. With it, the app
//! downloads one file, caches it next to its config, and
//! answers locally. Live detection stays as the fallback
//! for mods the catalog does not know yet.
//!
//! Privacy posture: the client downloads the whole index
//! and looks up locally — nothing about the user's library
//! ever leaves their machine.

use super::mod_hub::{Catalog, CatalogEntry};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::PathBuf;

/// Where the production catalog is published. The setting
/// can point elsewhere (a mirror, or Oracle later); an empty
/// setting disables fetching (a local cache still works).
pub const DEFAULT_CATALOG_URL: &str =
    "https://raw.githubusercontent.com/Jmilburn1/playground-catalog/main/catalog.json";

/// How old a cached catalog may get before a refresh is
/// attempted. ModHub versions change on human timescales;
/// a day is plenty fresh and keeps fetches rare.
pub const CACHE_TTL_HOURS: u64 = 24;

/// The cache lives beside config.toml:
/// ~/.config/fs-mod-manager/catalog-cache.json
pub fn cache_path() -> Option<PathBuf> {
    let home = dirs::home_dir()?;
    Some(
        home.join(".config")
            .join("fs-mod-manager")
            .join("catalog-cache.json"),
    )
}

/// The cached catalog, if one has ever been downloaded.
pub fn load_cache() -> Option<Catalog> {
    let path = cache_path()?;
    Catalog::load(&path).ok()
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn is_fresh(cat: &Catalog) -> bool {
    now_secs().saturating_sub(cat.generated_at) < CACHE_TTL_HOURS * 3600
}

/// The published catalog's file name (owner's naming,
/// 2026-10-19): the ModHub database is `catalog-hub.json`.
pub const HUB_FILE_NAME: &str = "catalog-hub.json";

/// The pre-rename published name. During the transition the
/// publisher writes BOTH names (dual-publish); readers
/// prefer the new one and fall back to this.
pub const LEGACY_FILE_NAME: &str = "catalog.json";

/// The URLs to try for a configured catalog URL, best
/// first: when the configured URL names the legacy file,
/// its `catalog-hub.json` sibling is preferred and the
/// configured URL is the fallback; a URL already naming
/// catalog-hub.json (or anything else) is used as-is.
pub fn fetch_urls(url: &str) -> Vec<String> {
    let trimmed = url.trim_end_matches('/');
    if let Some(prefix) = trimmed.strip_suffix(LEGACY_FILE_NAME) {
        // Only swap a whole final path segment, not a
        // longer name that merely ends the same way.
        if prefix.is_empty() || prefix.ends_with('/') {
            let hub = format!("{}{}", prefix, HUB_FILE_NAME);
            if hub != url {
                return vec![hub, url.to_string()];
            }
        }
    }
    vec![url.to_string()]
}

/// Download the catalog from `url`, replace the cache with
/// it, and return it. The catalog-hub/catalog file-name
/// preference (see `fetch_urls`) is applied: each candidate
/// is tried in order until one answers with a valid
/// catalog.
pub async fn fetch(url: &str) -> Result<Catalog> {
    let client = reqwest::Client::builder()
        .user_agent("Playground-FS-Mod-Manager/1.0 (catalog reader)")
        .build()
        .context("Could not build the catalog HTTP client")?;
    let mut last_err: Option<anyhow::Error> = None;
    for candidate in fetch_urls(url) {
        match fetch_one(&client, &candidate).await {
            Ok(cat) => {
                if let Some(path) = cache_path() {
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    // A cache write failure is not fatal — the
                    // in-memory copy is what this run needs.
                    let _ = cat.save(&path);
                }
                return Ok(cat);
            }
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("The catalog did not answer")))
}

/// One download attempt against one exact URL.
async fn fetch_one(client: &reqwest::Client, url: &str) -> Result<Catalog> {
    let text = client
        .get(url)
        .send()
        .await
        .context("The catalog did not answer")?
        .error_for_status()
        .context("The catalog server returned an error")?
        .text()
        .await
        .context("The catalog download was unreadable")?;
    let cat: Catalog =
        serde_json::from_str(&text).context("The downloaded catalog is not valid catalog JSON")?;
    Ok(cat)
}

/// The catalog to work with right now: a fresh cache wins;
/// a stale or missing cache triggers one fetch attempt; a
/// failed fetch falls back to the stale cache rather than
/// leaving the caller with nothing.
pub async fn ensure(url: &str) -> Option<Catalog> {
    let cached = load_cache();
    if let Some(cat) = &cached {
        if is_fresh(cat) {
            return cached;
        }
    }
    if url.trim().is_empty() {
        return cached;
    }
    match fetch(url).await {
        Ok(cat) => Some(cat),
        Err(_) => cached,
    }
}

/// stem -> entry lookup for a whole catalog. Entries are
/// cloned; a full ModHub index is a few thousand small
/// records, so this stays cheap.
pub fn by_stem(cat: &Catalog) -> HashMap<String, CatalogEntry> {
    cat.mods
        .iter()
        .map(|e| (e.stem.clone(), e.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hub_name_is_preferred_with_legacy_fallback() {
        let urls = fetch_urls(
            "https://raw.githubusercontent.com/Jmilburn1/playground-catalog/main/catalog.json",
        );
        assert_eq!(
            urls,
            vec![
                "https://raw.githubusercontent.com/Jmilburn1/playground-catalog/main/catalog-hub.json"
                    .to_string(),
                "https://raw.githubusercontent.com/Jmilburn1/playground-catalog/main/catalog.json"
                    .to_string(),
            ]
        );
    }

    #[test]
    fn hub_url_and_other_urls_pass_through() {
        assert_eq!(
            fetch_urls("https://example.com/main/catalog-hub.json"),
            vec!["https://example.com/main/catalog-hub.json".to_string()]
        );
        assert_eq!(
            fetch_urls("https://example.com/index.json"),
            vec!["https://example.com/index.json".to_string()]
        );
        // A longer file name merely ending in the legacy
        // name is not the catalog file — no sibling swap.
        assert_eq!(
            fetch_urls("https://example.com/my-catalog.json"),
            vec!["https://example.com/my-catalog.json".to_string()]
        );
    }
}
