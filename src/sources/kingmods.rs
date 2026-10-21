use crate::sources::{ModSearchResult, ModSource};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// KingMods update source — the third source.
///
/// A mod is linked by its KingMods page URL (stored in the
/// catalog-other store, keyed by file stem). The hunt-era
/// pages were said to carry a ModHub-style "Filename:" /
/// "Version:" info block, but that block does NOT exist in
/// the raw HTML — verified 2026-10-21 against live pages
/// and a September 2025 archive snapshot. What the page
/// does carry, in three consistent places that must agree
/// before any of them is believed:
///
/// - the Versions list: one `<li>` per release, its button
///   text `V<x.y.z.w>` followed by `<time datetime="...">`,
///   newest first;
/// - the header chip: the current `V<x.y.z.w>` beside the
///   downloads count, above the Versions list;
/// - the download button: the brand-styled anchor whose
///   text ends in `V<x.y.z.w>` (the verb is localized —
///   "Download", "Descarga", ...), pointing at the file's
///   real host — KingMods itself hosts nothing. A mirror
///   dropdown beside it also carries `Download V...`
///   items for OLDER versions on lagging mirror hosts;
///   only the brand button speaks for the current one.
///
/// The exact download file name appears only when that
/// external URL ends in the .zip name (GitHub release
/// assets do; itch / workupload / author-site links don't).
/// It is captured when present, and the fill refuses to
/// judge from it when it names a different file than the
/// library's — the project's identity rule is the exact
/// file name. If the page's signals disagree, parsing
/// fails loudly instead of guessing, like the ModHub leg.
pub struct KingModsSource;

impl KingModsSource {
    pub fn new() -> Self {
        Self
    }

    fn client(&self) -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder().user_agent(UA).build()?)
    }

    /// The linked page's current info, or None when the
    /// page is gone (404). A page that loads but doesn't
    /// parse is an error — the layout changed, and the
    /// user should hear that instead of a wrong version.
    pub async fn fetch_info(&self, url: &str) -> Result<Option<KingModsInfo>> {
        let resp = self
            .client()?
            .get(url)
            .send()
            .await
            .with_context(|| format!("Could not reach KingMods page {}", url))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        super::throttle::check_status("KingMods", resp.status())?;
        let html = resp
            .error_for_status()
            .with_context(|| format!("KingMods page failed: {}", url))?
            .text()
            .await
            .context("KingMods sent an unreadable page")?;
        match parse_page(&html) {
            Some(info) => Ok(Some(info)),
            None => anyhow::bail!(
                "The KingMods page {} didn't parse — its layout may have changed.",
                url
            ),
        }
    }
}

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// What a KingMods page reports about its mod.
#[derive(Debug, Clone, PartialEq)]
pub struct KingModsInfo {
    /// The mod's current version ("1.2.0.0").
    pub version: String,
    /// The exact .zip file name, when the download URL
    /// carries it. None for the common case — an external
    /// host whose link names no file.
    pub file_name: Option<String>,
    /// The download URL the page's button points at
    /// (always an external host), when one was found.
    pub download_url: Option<String>,
}

/// The Versions list's (version, release datetime) pairs,
/// in page order. Each entry's button is `V<x>` followed
/// by a `<time datetime="...">`; nothing else on the page
/// has that shape.
fn version_entries(html: &str) -> Vec<(String, String)> {
    let re = regex::Regex::new(r#"V(\d+(?:\.\d+)*)\s*<time\s+datetime="([^"]+)""#).unwrap();
    re.captures_iter(html)
        .map(|c| (c[1].to_string(), c[2].to_string()))
        .collect()
}

/// The header chip's version: the `V<x>` that closes the
/// info block beside the downloads count. It sits above
/// the Versions list in every observed layout, so the
/// search stops where the list begins — a version-looking
/// chip further down belongs to some other mod's card.
fn header_version(html: &str, list_start: Option<usize>) -> Option<String> {
    let limit = list_start.unwrap_or(html.len());
    let re = regex::Regex::new(r"</svg>\s*V(\d+(?:\.\d+)*)\s*</div>").unwrap();
    re.captures(&html[..limit.min(html.len())])
        .map(|c| c[1].to_string())
}

/// The primary download buttons: (version, URL) pairs.
/// The button is the brand-styled anchor; its body is an
/// (inline SVG) icon plus a localized verb and the
/// version — the SVGs are stripped first so the text
/// sits next to the tag, and the first version-shaped
/// token in the text that follows is the button's. The
/// mirror dropdown's `Download V...` items are NOT this
/// signal: they point at mirror hosts still serving
/// older versions.
fn download_buttons(html: &str) -> Vec<(String, String)> {
    let svg_re = regex::Regex::new(r"(?s)<svg.*?</svg>").unwrap();
    let flat = svg_re.replace_all(html, "");
    let tag_re = regex::Regex::new(r"<a\s[^>]*>").unwrap();
    let href_re = regex::Regex::new(r#"href="([^"]+)""#).unwrap();
    let ver_re = regex::Regex::new(r"V(\d+(?:\.\d+)*)").unwrap();
    let mut out = Vec::new();
    for m in tag_re.find_iter(&flat) {
        let tag = m.as_str();
        if !tag.contains("btn-brand") {
            continue;
        }
        let Some(href) = href_re.captures(tag).map(|c| c[1].to_string()) else {
            continue;
        };
        let after = &flat[m.end()..flat.len().min(m.end() + 120)];
        if let Some(v) = ver_re.captures(after) {
            out.push((v[1].to_string(), href));
        }
    }
    out
}

/// Parse a KingMods mod page. The versions list is the
/// primary signal (its latest entry by release datetime);
/// the header chip and the download buttons must agree
/// with it when present, or the page is not believed.
/// Without a list, the chip and the buttons must agree
/// with each other. None when the page carries no
/// believable version at all.
pub fn parse_page(html: &str) -> Option<KingModsInfo> {
    let entries = version_entries(html);
    let list_start = entries.first().and_then(|_| {
        let re = regex::Regex::new(r"V\d+(?:\.\d+)*\s*<time\s+datetime=").unwrap();
        re.find(html).map(|m| m.start())
    });
    // Latest = the entry with the newest release datetime
    // (ISO strings compare chronologically); page order
    // breaks ties.
    let list_version = entries
        .iter()
        .max_by(|a, b| a.1.cmp(&b.1))
        .map(|e| e.0.clone());
    let chip = header_version(html, list_start);
    let buttons = download_buttons(html);

    let version = match (&list_version, &chip) {
        (Some(v), Some(c)) if c != v => return None,
        (Some(v), _) => v.clone(),
        (None, Some(c)) => {
            // No list: one signal is not a version — the
            // chip needs the buttons to corroborate it.
            if !buttons.is_empty() && buttons.iter().all(|(bv, _)| bv == c) {
                c.clone()
            } else {
                return None;
            }
        }
        (None, None) => return None,
    };
    if buttons.iter().any(|(bv, _)| *bv != version) {
        return None;
    }

    // The download file name, when the primary button's
    // URL carries it: the URL's last segment, ending in
    // .zip (sharemods-style hosts append ".html" to the
    // file's name — that suffix is theirs, not the
    // file's). A URL naming no file claims no name.
    let download_url = buttons.first().map(|(_, u)| u.clone());
    let file_name = download_url.as_ref().and_then(|url| {
        let path = url.split(['?', '#']).next().unwrap_or(url);
        let mut seg = path.rsplit('/').next().unwrap_or_default();
        if seg.to_lowercase().ends_with(".zip.html") {
            seg = &seg[..seg.len() - ".html".len()];
        }
        if seg.to_lowercase().ends_with(".zip") && !seg.is_empty() {
            Some(seg.to_string())
        } else {
            None
        }
    });

    Some(KingModsInfo {
        version,
        file_name,
        download_url,
    })
}

// The original synchronous trait stays for shape; the real
// work goes through the async fetch above.
impl ModSource for KingModsSource {
    fn name(&self) -> &str {
        "KingMods"
    }

    fn search(&self, _query: &str, _game_version: &str) -> Result<Vec<ModSearchResult>> {
        Ok(Vec::new())
    }

    fn download(&self, _mod_id: &str, _dest: PathBuf) -> Result<PathBuf> {
        Err(anyhow::anyhow!("Use the update check flow"))
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

    /// One-shot loopback stub answering the given status —
    /// the tripwire injection seam (see mod_hub's tests).
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
    async fn a_429_from_kingmods_is_a_throttle_error() {
        let src = KingModsSource::new();
        let err = src
            .fetch_info(&stub_status("429 Too Many Requests"))
            .await
            .unwrap_err();
        assert_eq!(crate::sources::throttle::throttle_status(&err), Some(429));
    }

    // Shaped like the live page (2026-10-21): a header
    // chip beside the downloads count, a Versions list
    // with datetimes, and download buttons whose anchors
    // point at the file's external host.
    const PAGE_EXTERNAL: &str = r#"
        <div class="flex">
            <svg class="inline-block" viewBox="0 0 24 24"><path d="M13.26 3C8.17 2.86 4 6.95 4 12"/></svg>
                        V1.2.0.0
                    </div>
                    <div aria-label="11 808 downloads" class="flex"></div>
        <ul>
            <li x-data="{ expanded: true }" class="rounded-20 overflow-hidden bg-surface-1">
                <button type="button" class="w-full p-30">
                    V1.2.0.0
                    <time datetime="2026-08-24T09:16:10Z" class="text-12">August 24, 2026</time>
                </button>
                <div x-show="expanded"><p>- Added new wheels</p></div>
            </li>
            <li x-data="{ expanded: false }" class="rounded-20">
                <button type="button" class="w-full p-30">
                    V1.0.0.1
                    <time datetime="2026-03-31T09:01:16Z" class="text-12">March 31, 2026</time>
                </button>
            </li>
        </ul>
        <a href="https://crownexmods.itch.io/fs25-1976-chevrolet-k30" target="_blank" class="btn btn-lg btn-brand flex-grow">
            <svg viewBox="0 0 24 24"><path d="M16.59,9H15V4"/></svg>
                        Download V1.2.0.0
                        <span class="ml-3">(crownexmods.itch.io)</span>
                    </a>
        <a href="https://mirror.example/fs25-1976-chevrolet-k30" target="_blank" class="dropdown-item">
                        Download V1.0.0.1
                    </a>
    "#;

    #[test]
    fn page_with_external_download_parses() {
        let info = parse_page(PAGE_EXTERNAL).expect("page should parse");
        assert_eq!(info.version, "1.2.0.0");
        // The host's page URL names no file.
        assert_eq!(info.file_name, None);
        assert_eq!(
            info.download_url.as_deref(),
            Some("https://crownexmods.itch.io/fs25-1976-chevrolet-k30")
        );
    }

    #[test]
    fn page_with_zip_download_names_the_file() {
        // GitHub-hosted download (the HudHider shape): the
        // button's URL ends in the exact file name.
        let html = PAGE_EXTERNAL.replace(
            "https://crownexmods.itch.io/fs25-1976-chevrolet-k30",
            "https://github.com/kingmodsnet/FS25_HudHider/releases/download/V1.2.0.0/FS25_HudHider_KingMods.zip",
        );
        let info = parse_page(&html).expect("page should parse");
        assert_eq!(info.version, "1.2.0.0");
        assert_eq!(info.file_name.as_deref(), Some("FS25_HudHider_KingMods.zip"));
        assert!(info
            .download_url
            .as_deref()
            .unwrap()
            .ends_with("FS25_HudHider_KingMods.zip"));
    }

    #[test]
    fn latest_entry_wins_by_date_not_position() {
        // A page whose list order can't be trusted: the
        // newest datetime still names the latest version.
        let html = r#"
            <ul>
                <li><button>V1.0.0.0 <time datetime="2025-01-01T00:00:00Z">x</time></button></li>
                <li><button>V1.3.0.0 <time datetime="2025-06-01T00:00:00Z">x</time></button></li>
                <li><button>V1.2.0.0 <time datetime="2025-03-01T00:00:00Z">x</time></button></li>
            </ul>
        "#;
        let info = parse_page(html).expect("page should parse");
        assert_eq!(info.version, "1.3.0.0");
    }

    #[test]
    fn disagreeing_signals_do_not_parse() {
        // Chip says one thing, the list another: the page
        // is not believed, exactly like the ModHub leg.
        let html = PAGE_EXTERNAL.replace(
            "<svg class=\"inline-block\" viewBox=\"0 0 24 24\"><path d=\"M13.26 3C8.17 2.86 4 6.95 4 12\"/></svg>\n                        V1.2.0.0",
            "<svg class=\"inline-block\" viewBox=\"0 0 24 24\"><path d=\"M13.26 3C8.17 2.86 4 6.95 4 12\"/></svg>\n                        V9.9.9.9",
        );
        assert!(parse_page(&html).is_none());
    }

    #[test]
    fn page_without_version_block_does_not_parse() {
        assert!(parse_page("<html><body>nothing here</body></html>").is_none());
        // A lone chip with no list and no button is one
        // signal, not a version: not believed.
        let html = r#"<div><svg viewBox="0 0 24 24"></svg> V1.0.0.0 </div>"#;
        assert!(parse_page(html).is_none());
    }
}


