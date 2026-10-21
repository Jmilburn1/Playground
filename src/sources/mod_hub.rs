use crate::sources::{ModSearchResult, ModSource};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// GIANTS ModHub update source — the second source (Phase 4).
///
/// ModHub has no documented public API, but its pages are a
/// clean interface: every mod has a stable page
/// (farming-simulator.com/mod.php?mod_id=<id>&title=fs2025)
/// whose info table carries the mod's Filename and current
/// Version as labeled fields, and whose DOWNLOAD button links
/// the file directly on the GIANTS CDN. The CDN refuses
/// referer-less requests, so downloads send the mod page as
/// the Referer — exactly what a browser does.
///
/// A mod is linked by its ModHub mod_id (stored in the config
/// keyed by file stem, like the GitHub links). The check reads
/// the page, the install downloads the CDN file; the manager
/// validates and swaps it in, same as the GitHub flow. If the
/// page layout ever changes, parsing fails loudly instead of
/// guessing.
/// The production ModHub host. The page paths below are
/// appended to it.
pub const DEFAULT_BASE: &str = "https://www.farming-simulator.com";

pub struct ModHubSource {
    /// Scheme + host the pages are fetched from. Production
    /// is always DEFAULT_BASE; the seam exists so tests can
    /// point the source at a local stub (e.g. one that
    /// answers 403, to prove the throttling tripwire without
    /// ever provoking the real GIANTS).
    base: String,
}

impl ModHubSource {
    pub fn new() -> Self {
        Self {
            base: DEFAULT_BASE.to_string(),
        }
    }

    /// A source speaking to a different host (tests only).
    #[cfg(test)]
    pub fn with_base(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_string(),
        }
    }

    /// This source's page URL for a mod.
    fn page_url(&self, mod_id: &str) -> String {
        format!("{}/mod.php?mod_id={}&title=fs2025", self.base, mod_id)
    }

    fn client(&self) -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder().user_agent(UA).build()?)
    }

    /// Candidate mod_ids from a ModHub search, in the page's
    /// own (relevance) order, deduplicated. The search box's
    /// parameter is `searchMod` — plain `search=` is silently
    /// ignored by the site and returns the generic listing
    /// (verified the hard way while building this).
    pub async fn search_candidates(&self, query: &str) -> Result<Vec<String>> {
        let url = format!(
            "{}/mods.php?title=fs2025&searchMod={}",
            self.base,
            encode_query(query)
        );
        let resp = self
            .client()?
            .get(&url)
            .send()
            .await
            .context("ModHub search did not answer")?;
        super::throttle::check_status("GIANTS ModHub", resp.status())?;
        let html = resp
            .error_for_status()
            .context("ModHub search failed")?
            .text()
            .await
            .context("ModHub sent an unreadable search page")?;
        Ok(extract_ids(&html))
    }

    /// The mod_ids on one FS25 listing page (newest first,
    /// page is 0-based) — the catalog indexer's raw material.
    /// An empty vec means the walk has run past the end.
    pub async fn listing_ids(&self, page: u32) -> Result<Vec<String>> {
        let url = format!("{}/mods.php?title=fs2025&page={}", self.base, page);
        let resp = self
            .client()?
            .get(&url)
            .send()
            .await
            .context("ModHub listing did not answer")?;
        super::throttle::check_status("GIANTS ModHub", resp.status())?;
        let html = resp
            .error_for_status()
            .context("ModHub listing failed")?
            .text()
            .await
            .context("ModHub sent an unreadable listing page")?;
        Ok(extract_ids(&html))
    }

    /// Auto-detect the ModHub mod_id for one local mod file:
    /// search by the file stem (then by the mod's title when
    /// that differs), and verify each candidate on its own
    /// page — the link is made only when the candidate's
    /// Filename field equals the local file name exactly
    /// (case-insensitive). A wrong guess is worse than no link,
    /// so unverified candidates are never returned. Candidates
    /// are capped per query and fetched `width` at a time, so
    /// callers can trade politeness against speed (the batch
    /// detector runs several mods concurrently with a narrow
    /// width; the total request pressure stays browser-like).
    pub async fn find_mod_id(
        &self,
        file_name: &str,
        title: &str,
        width: usize,
    ) -> Result<Option<String>> {
        let stem = file_name
            .strip_suffix(".zip")
            .unwrap_or(file_name)
            .to_string();
        let mut queries = vec![stem.clone()];
        let t = title.trim();
        if !t.is_empty() && t.to_lowercase() != stem.to_lowercase() {
            queries.push(t.to_string());
        }
        let target = file_name.to_lowercase();
        let width = width.max(1);
        for query in &queries {
            let candidates = self.search_candidates(query).await?;
            let top: Vec<String> = candidates.iter().take(12).cloned().collect();
            for chunk in top.chunks(width) {
                let results = futures::future::join_all(
                    chunk.iter().map(|id| self.fetch_info(id)),
                )
                .await;
                for (id, res) in chunk.iter().zip(results) {
                    if let Ok(Some(info)) = res {
                        if info.file_name.to_lowercase() == target {
                            return Ok(Some(id.clone()));
                        }
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            }
        }
        Ok(None)
    }
}

/// Minimal query encoding for ModHub searches: spaces become
/// '+', the characters mod names actually use pass through,
/// everything else is percent-encoded.
fn encode_query(q: &str) -> String {
    let mut out = String::new();
    for ch in q.chars() {
        match ch {
            ' ' => out.push('+'),
            'A'..='Z' | 'a'..='z' | '0'..='9' | '_' | '-' | '.' => out.push(ch),
            _ => {
                let mut buf = [0u8; 4];
                for b in ch.encode_utf8(&mut buf).as_bytes() {
                    out.push_str(&format!("%{:02X}", b));
                }
            }
        }
    }
    out
}

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// Every mod_id linked from a ModHub page (search results or
/// a listing), in page order, deduplicated.
fn extract_ids(html: &str) -> Vec<String> {
    let re = regex::Regex::new(r"mod\.php\?mod_id=(\d+)").unwrap();
    let mut seen = std::collections::HashSet::new();
    let mut ids = Vec::new();
    for c in re.captures_iter(html) {
        let id = c[1].to_string();
        if seen.insert(id.clone()) {
            ids.push(id);
        }
    }
    ids
}

/// One mod's record in the published catalog — everything a
/// client needs to answer "is there a newer version, and
/// where's the file" without contacting ModHub itself.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CatalogEntry {
    /// File stem, lowercase, no .zip — the client's join key.
    pub stem: String,
    /// The exact file name ModHub serves.
    pub file_name: String,
    /// The ModHub mod_id.
    pub modhub_id: String,
    /// ModHub's current version for the mod.
    pub version: String,
    /// The direct CDN download URL from the mod's page.
    pub download_url: String,
    /// ModHub's "Released" date for the mod, exactly as the
    /// mod page's info table reports it (e.g. "28.04.2026").
    /// That is the label GIANTS shows; whether their backend
    /// treats it as a creation date is not visible from the
    /// page, so it is stored verbatim under their label.
    /// Empty on records written before this field existed —
    /// the complete-walk build (`catalog-build --full`)
    /// backfills it for every record it re-fetches.
    #[serde(default)]
    pub released: String,
    /// Unix seconds when this record was last verified
    /// against ModHub — drives the indexer's refresh cycle:
    /// records older than the stale window get re-fetched, so
    /// version bumps are picked up without re-walking
    /// everything every run.
    #[serde(default)]
    pub checked_at: u64,
}

/// The catalog file: a flat, self-describing index of ModHub,
/// published as one static JSON so any static host (GitHub
/// today, anywhere tomorrow) can serve it and clients can
/// answer update checks locally.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Catalog {
    /// Format version, for future shape changes.
    pub format: u32,
    /// Unix seconds when this file was generated.
    pub generated_at: u64,
    pub mods: Vec<CatalogEntry>,
}

impl Catalog {
    pub fn empty() -> Self {
        Catalog {
            format: 1,
            generated_at: 0,
            mods: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Catalog> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("Could not read catalog {:?}", path))?;
        serde_json::from_str(&text).context("The catalog file is not valid catalog JSON")
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = serde_json::to_string_pretty(self).context("Catalog did not serialize")?;
        // Write-then-rename so a killed run never leaves a
        // half-written catalog behind.
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text)
            .with_context(|| format!("Could not write catalog {:?}", tmp))?;
        std::fs::rename(&tmp, path)
            .with_context(|| format!("Could not replace catalog {:?}", path))?;
        Ok(())
    }
}

/// The public page for a ModHub mod (FS25 section), on the
/// production host.
pub fn page_url(mod_id: &str) -> String {
    format!("{}/mod.php?mod_id={}&title=fs2025", DEFAULT_BASE, mod_id)
}

/// Normalize a user-typed ModHub reference to the numeric
/// mod_id. Accepts the bare id, a full mod.php URL (the id is
/// read from its query), or any string containing "mod_id=".
pub fn normalize_modhub(input: &str) -> Option<String> {
    let s = input.trim();
    if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()) {
        return Some(s.to_string());
    }
    let marker = "mod_id=";
    let start = s.find(marker)? + marker.len();
    let digits: String = s[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        Some(digits)
    }
}

/// What a ModHub page reports about its mod.
#[derive(Debug, Clone, PartialEq)]
pub struct ModHubInfo {
    /// The mod's current version on ModHub ("1.1.0.0").
    pub version: String,
    /// The exact .zip filename ModHub serves.
    pub file_name: String,
    /// The direct CDN download URL from the page.
    pub download_url: String,
    /// ModHub's "Released" date as the page reports it
    /// (e.g. "28.04.2026"); empty when the page carries no
    /// Released field — a missing date never fails a parse
    /// whose version/filename did read.
    pub released: String,
}

/// Extract one labeled value from the page's info table: rows
/// are `<div class="table-cell"><b>LABEL</b></div>` followed by
/// `<div class="table-cell">VALUE</div>`.
fn table_value(html: &str, label: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(
        r#"<b>{}</b></div>\s*<div class="table-cell">([^<]+)</div>"#,
        regex::escape(label)
    ))
    .ok()?;
    re.captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().trim().to_string())
}

/// Parse a ModHub mod page. The download URL is the page's own
/// CDN link; the filename falls back to that URL's last segment
/// when the table is missing it. None when the page doesn't
/// carry the fields — the caller reports that plainly rather
/// than treating an unparseable page as "no update".
pub fn parse_page(html: &str) -> Option<ModHubInfo> {
    let version = table_value(html, "Version")?;
    let url_re =
        regex::Regex::new(r#"href="(https://cdn\d+\.giants-software\.com/modHub/storage/[^"]+?\.zip)""#)
            .ok()?;
    let download_url = url_re
        .captures(html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())?;
    let file_name = table_value(html, "Filename").unwrap_or_else(|| {
        download_url
            .rsplit('/')
            .next()
            .unwrap_or_default()
            .to_string()
    });
    if version.is_empty() || file_name.is_empty() {
        return None;
    }
    let released = table_value(html, "Released").unwrap_or_default();
    Some(ModHubInfo {
        version,
        file_name,
        download_url,
        released,
    })
}

impl ModHubSource {
    /// The mod's current ModHub info, or None when the page is
    /// gone (404). A page that loads but doesn't parse is an
    /// error — ModHub's layout changed, and the user should
    /// hear that instead of a wrong "up to date".
    pub async fn fetch_info(&self, mod_id: &str) -> Result<Option<ModHubInfo>> {
        let url = self.page_url(mod_id);
        let resp = reqwest::Client::builder()
            .user_agent(UA)
            .build()?
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Could not reach ModHub for mod {}", mod_id))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        super::throttle::check_status("GIANTS ModHub", resp.status())?;
        let html = resp
            .error_for_status()
            .with_context(|| format!("ModHub page failed for mod {}", mod_id))?
            .text()
            .await
            .context("ModHub sent an unreadable page")?;
        match parse_page(&html) {
            Some(info) => Ok(Some(info)),
            None => anyhow::bail!(
                "The ModHub page for mod {} didn't parse — its layout may have changed.",
                mod_id
            ),
        }
    }

    /// Stream a ModHub file to `dest`. The CDN requires the
    /// mod page as Referer; without it the answer is 403.
    pub async fn download_to(&self, url: &str, referer: &str, dest: &Path) -> Result<u64> {
        use futures::StreamExt;
        let resp = reqwest::Client::builder()
            .user_agent(UA)
            .build()?
            .get(url)
            .header("Referer", referer)
            .send()
            .await
            .context("Download did not start")?
            .error_for_status()
            .context("Download was refused (the CDN wants the mod page as referer)")?;
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

// The original synchronous trait stays for shape; ModHub's real
// work goes through the async methods above.
impl ModSource for ModHubSource {
    fn name(&self) -> &str {
        "Mod Hub"
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
    fn modhub_references_normalize() {
        assert_eq!(normalize_modhub("304979").as_deref(), Some("304979"));
        assert_eq!(
            normalize_modhub(
                "https://www.farming-simulator.com/mod.php?mod_id=304979&title=fs2025"
            )
            .as_deref(),
            Some("304979")
        );
        assert_eq!(normalize_modhub("no id here"), None);
        assert_eq!(normalize_modhub(""), None);
    }

    #[test]
    fn page_parses_like_the_real_thing() {
        // Shaped like the live page: labeled table rows for
        // Filename/Version and a CDN DOWNLOAD anchor.
        let html = r#"
            <div class="table-row">
                <div class="table-cell"><b>Filename</b></div>
                <div class="table-cell">FS25_Concrete_Curb_Pack.zip</div>
            </div>
            <div class="table-row">
                <div class="table-cell"><b>Version</b></div>
                <div class="table-cell">1.1.0.0</div>
            </div>
            <div class="table-row">
                <div class="table-cell"><b>Released</b></div>
                <div class="table-cell">28.04.2026</div>
            </div>
            <a href="https://cdn26.giants-software.com/modHub/storage/00304979/FS25_Concrete_Curb_Pack.zip" class="button">DOWNLOAD</a>
        "#;
        let info = parse_page(html).expect("page should parse");
        assert_eq!(info.version, "1.1.0.0");
        assert_eq!(info.file_name, "FS25_Concrete_Curb_Pack.zip");
        assert!(info.download_url.contains("cdn26.giants-software.com"));
        assert_eq!(info.released, "28.04.2026");
    }

    #[test]
    fn catalog_records_without_released_still_load() {
        // Catalogs published before the Released field existed
        // carry no "released" key; they must still parse, with
        // the date empty until a --full walk backfills it.
        let text = r#"{
            "format": 1,
            "generated_at": 1792254539,
            "mods": [{
                "stem": "fs25_000_datadump",
                "file_name": "FS25_000_DataDump.zip",
                "modhub_id": "303452",
                "version": "1.0.0.0",
                "download_url": "https://cdn27.giants-software.com/modHub/storage/00303452/FS25_000_DataDump.zip",
                "checked_at": 1792251330
            }]
        }"#;
        let cat: Catalog = serde_json::from_str(text).expect("old catalog should load");
        assert_eq!(cat.mods.len(), 1);
        assert_eq!(cat.mods[0].released, "");
    }

    #[test]
    fn page_without_fields_does_not_parse() {
        assert!(parse_page("<html><body>nothing here</body></html>").is_none());
    }

    /// A one-shot HTTP stub on a loopback port that answers
    /// every request with the given status line — the
    /// injection seam for the tripwire tests (a real 403
    /// from GIANTS is never provoked).
    fn stub_status(status_line: &str) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let line = status_line.to_string();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                    line
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn a_403_from_modhub_is_a_throttle_error() {
        let src = ModHubSource::with_base(&stub_status("403 Forbidden"));
        let err = src.fetch_info("123").await.unwrap_err();
        assert_eq!(crate::sources::throttle::throttle_status(&err), Some(403));
    }

    #[tokio::test]
    async fn a_429_from_modhub_search_is_a_throttle_error() {
        let src = ModHubSource::with_base(&stub_status("429 Too Many Requests"));
        let err = src.search_candidates("anything").await.unwrap_err();
        assert_eq!(crate::sources::throttle::throttle_status(&err), Some(429));
    }

    #[test]
    fn queries_encode_for_the_search_box() {
        assert_eq!(encode_query("FS25_Concrete_Curb_Pack"), "FS25_Concrete_Curb_Pack");
        assert_eq!(encode_query("Concrete Curbs Pack"), "Concrete+Curbs+Pack");
        assert_eq!(encode_query("Bäume & Co"), "B%C3%A4ume+%26+Co");
    }
}
