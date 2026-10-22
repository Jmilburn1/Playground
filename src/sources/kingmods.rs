use crate::sources::{ModSearchResult, ModSource};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

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
///
/// Downloads (v1.5.2) follow the button's URL to the file
/// host and walk its free-download steps — see
/// `download_to_reporting`. Before this leg existed,
/// KingMods offers fell through to the GitHub downloader
/// and every install died as "Download was refused": the
/// URL is a host page, not a file, and the host never
/// accepted the GitHub lane's request.
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

    /// A client for the download flow: detection's browser
    /// UA, plus a cookie store — the file hosts run their
    /// free-download steps as one session, and the steps
    /// only line up when the session carries across them.
    /// One client serves a whole download, so the host sees
    /// one session on one connection throughout.
    fn download_client(&self) -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder()
            .user_agent(UA)
            .cookie_store(true)
            .build()?)
    }

    /// Stream the file behind a KingMods download URL to
    /// `dest`. `page` is the mod's KingMods page — the
    /// referer the chain starts from.
    pub async fn download_to(&self, url: &str, page: &str, dest: &Path) -> Result<u64> {
        self.download_to_reporting(url, page, dest, &mut |_, _| {})
            .await
    }

    /// `download_to` with progress: `report` hears (bytes
    /// so far, total when the host states one) as the stream
    /// lands, gated by ProgressGate — the same discipline
    /// (and the same ~150 ms / whole-percent cadence) as the
    /// ModHub and GitHub lanes.
    ///
    /// The URL a KingMods button points at is rarely the
    /// file: it is the file's own page on a file host
    /// (ShareMods and its XFileSharing kin — verified live
    /// 2026-10-22 against the Feliz page). The flow follows
    /// that host's free-download steps the way a browser
    /// would: fetch the page with the KingMods page as
    /// referer, submit the page's download form, follow to
    /// the next step until a response IS the file, then
    /// stream it to `dest`.
    ///
    /// Failure is specific, never a fall-through: a 403/429
    /// from the host trips the throttle error; the host's
    /// own error banner is quoted; a page whose shape isn't
    /// a download step is a resolution failure; and a host
    /// that gates its free downloads behind a browser check
    /// (Cloudflare Turnstile and kin) is named as exactly
    /// that — the token only a browser can mint is the one
    /// step no HTTP client can take.
    pub async fn download_to_reporting(
        &self,
        url: &str,
        page: &str,
        dest: &Path,
        report: &mut (dyn FnMut(u64, Option<u64>) + Send),
    ) -> Result<u64> {
        use futures::StreamExt;
        let client = self.download_client()?;
        let host = host_name(url);
        enum Step {
            Get(String),
            Post(String, Vec<(String, String)>),
        }
        let mut step = Step::Get(url.to_string());
        let mut referer = page.to_string();
        let mut posts = 0u32;
        let resp = loop {
            if posts > 3 {
                anyhow::bail!(
                    "The file host ({}) kept answering pages instead of the file — its download flow isn't one Playground can follow",
                    host
                );
            }
            let r = match &step {
                Step::Get(u) => client
                    .get(u)
                    .header("Referer", &referer)
                    .send()
                    .await
                    .with_context(|| {
                        format!("Could not reach the file host ({})", host)
                    })?,
                Step::Post(u, fields) => client
                    .post(u)
                    .header("Referer", &referer)
                    .form(fields)
                    .send()
                    .await
                    .with_context(|| {
                        format!(
                            "The file host ({}) did not answer the download step",
                            host
                        )
                    })?,
            };
            // The tripwire applies to the host too: a
            // 403/429 is "stop", in the typed shape every
            // source uses.
            super::throttle::check_status("KingMods file host", r.status())?;
            if !r.status().is_success() {
                anyhow::bail!(
                    "The file host ({}) refused the download (HTTP {})",
                    host,
                    r.status().as_u16()
                );
            }
            if is_file_response(&r) {
                break r;
            }
            // A page, then: read it and resolve the next
            // step. The URL it landed on (redirects followed)
            // is the base the step resolves against and the
            // referer the next request carries.
            let landed = r.url().to_string();
            let html = r
                .text()
                .await
                .context("The file host sent an unreadable page")?;
            if let Some(banner) = host_error_banner(&html) {
                if banner.to_lowercase().contains("captcha") {
                    anyhow::bail!(browser_check_message(&host));
                }
                anyhow::bail!("The file host ({}) said: {}", host, banner);
            }
            let Some(form) = parse_host_form(&html) else {
                if demands_browser_check(&html) {
                    anyhow::bail!(browser_check_message(&host));
                }
                anyhow::bail!(
                    "The download link led to a page on {} that isn't a file and carries no download step Playground can follow",
                    host
                );
            };
            if posts >= 1 && demands_browser_check(&html) {
                // One step was already taken and the host is
                // still holding the next behind its check:
                // the check is the whole story.
                anyhow::bail!(browser_check_message(&host));
            }
            referer = landed.clone();
            posts += 1;
            step = Step::Post(resolve_action(&landed, &form.action), form.fields);
        };
        let total = resp.content_length();
        let mut file = tokio::fs::File::create(dest)
            .await
            .with_context(|| format!("Could not create {:?}", dest))?;
        let mut stream = resp.bytes_stream();
        let mut written: u64 = 0;
        let mut gate = super::ProgressGate::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("Download interrupted")?;
            tokio::io::AsyncWriteExt::write_all(&mut file, &chunk)
                .await
                .context("Could not write the downloaded file")?;
            written += chunk.len() as u64;
            if gate.should_report(written, total) {
                report(written, total);
            }
        }
        tokio::io::AsyncWriteExt::flush(&mut file)
            .await
            .context("Could not finish the downloaded file")?;
        report(written, total);
        Ok(written)
    }
}

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// The host part of a URL, for error messages
/// ("sharemods.com"); the URL itself when it doesn't
/// parse.
fn host_name(url: &str) -> String {
    reqwest::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(|h| h.to_string()))
        .unwrap_or_else(|| url.to_string())
}

/// The failure a browser-check wall is reported as: the
/// host's gate is named, and so is the way around it.
fn browser_check_message(host: &str) -> String {
    format!(
        "The file host ({}) guards its free downloads with a browser check (Cloudflare Turnstile) that Playground can't complete — this one has to be downloaded in a browser from the KingMods page",
        host
    )
}

/// True when a response is the file itself rather than
/// another page of the host's flow: an attachment
/// disposition, or any content type that isn't HTML.
fn is_file_response(resp: &reqwest::Response) -> bool {
    if let Some(cd) = resp.headers().get(reqwest::header::CONTENT_DISPOSITION) {
        if let Ok(s) = cd.to_str() {
            if s.to_lowercase().contains("attachment") {
                return true;
            }
        }
    }
    match resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
    {
        Some(ct) => {
            let ct = ct.trim().to_lowercase();
            !ct.starts_with("text/html") && !ct.starts_with("application/xhtml+xml")
        }
        None => false,
    }
}

/// True when a host page carries a browser-check widget
/// (Cloudflare Turnstile, reCAPTCHA, hCaptcha) — the step
/// only a real browser can take.
pub fn demands_browser_check(html: &str) -> bool {
    let lower = html.to_lowercase();
    lower.contains("cf-turnstile")
        || lower.contains("g-recaptcha")
        || lower.contains("h-captcha")
        || lower.contains("challenges.cloudflare.com/turnstile")
}

/// The host's own complaint, when the page carries one of
/// the XFileSharing family's known error banners. The
/// banner is returned in its canonical spelling so the
/// failure the user reads is the failure the host
/// reported.
pub fn host_error_banner(html: &str) -> Option<String> {
    const PHRASES: [&str; 4] = [
        "Wrong captcha",
        "Wrong IP address",
        "File not found",
        "No such file",
    ];
    let lower = html.to_lowercase();
    for phrase in PHRASES {
        if lower.contains(&phrase.to_lowercase()) {
            return Some(phrase.to_string());
        }
    }
    None
}

/// One file-host download form: where it posts (its
/// action, possibly empty for "this same URL") and its
/// fields, in page order.
#[derive(Debug, Clone, PartialEq)]
pub struct HostForm {
    pub action: String,
    pub fields: Vec<(String, String)>,
}

/// Extract a host page's download form: the form carrying
/// an `op` field when one exists (the XFileSharing shape),
/// else the first POST form on the page. Every named input
/// rides along, hidden or not — the steps check them all.
pub fn parse_host_form(html: &str) -> Option<HostForm> {
    let form_re = regex::Regex::new(r"(?is)<form\b([^>]*)>(.*?)</form>").unwrap();
    let mut first_post: Option<HostForm> = None;
    for cap in form_re.captures_iter(html) {
        let fields = form_fields(&cap[2]);
        if fields.is_empty() {
            continue;
        }
        let form = HostForm {
            action: attr_value(&cap[1], "action").unwrap_or_default(),
            fields,
        };
        if form.fields.iter().any(|(n, _)| n == "op") {
            return Some(form);
        }
        let method = attr_value(&cap[1], "method").unwrap_or_default();
        if first_post.is_none() && method.eq_ignore_ascii_case("post") {
            first_post = Some(form);
        }
    }
    first_post
}

/// A form body's named inputs, in page order: the name
/// and the value the page set (empty when it set none).
fn form_fields(body: &str) -> Vec<(String, String)> {
    let input_re = regex::Regex::new(r"(?is)<input\b[^>]*>").unwrap();
    let mut out = Vec::new();
    for m in input_re.find_iter(body) {
        if let Some(name) = attr_value(m.as_str(), "name") {
            let value = attr_value(m.as_str(), "value").unwrap_or_default();
            out.push((name, value));
        }
    }
    out
}

/// One attribute's value from a tag (double- or
/// single-quoted), when the attribute is there.
fn attr_value(tag: &str, attr: &str) -> Option<String> {
    let re = regex::Regex::new(&format!(
        r#"(?i)\b{}\s*=\s*(?:"([^"]*)"|'([^']*)')"#,
        regex::escape(attr)
    ))
    .ok()?;
    re.captures(tag)
        .and_then(|c| c.get(1).or_else(|| c.get(2)))
        .map(|m| m.as_str().to_string())
}

/// Resolve a form's action against the page it was found
/// on: an empty action posts back to the same URL.
fn resolve_action(page_url: &str, action: &str) -> String {
    let action = action.trim();
    if action.is_empty() {
        return page_url.to_string();
    }
    reqwest::Url::parse(page_url)
        .and_then(|base| base.join(action))
        .map(|u| u.to_string())
        .unwrap_or_else(|_| action.to_string())
}

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

    // ---- The download leg --------------------------------
    //
    // The host fixtures below are shaped from the live
    // ShareMods page for FS25_Feliz.zip, captured
    // 2026-10-22: the F1 form and its hidden fields are the
    // page's own, trimmed of ads and chrome. The live page
    // also embeds a Cloudflare Turnstile widget above the
    // button (in the fixture, as on the page).

    const HOST_PAGE_CHECKED: &str = r#"<!DOCTYPE html>
<html><head><title>Download FS25 Feliz zip</title></head><body>
<div class="download__filename-content">
  <strong>FS25_Feliz.zip</strong> <span>(1.2 GB)</span>
</div>
<div class="text-center mt-4 mb-4"><div class="d-inline-block">
<script src="https://challenges.cloudflare.com/turnstile/v0/api.js" defer></script>
<div class="cf-turnstile" data-sitekey="0x4AAAAAADFrOJ1ncwtEU1yA"></div>
</div></div>
<form name="F1" method="POST" action="">
  <input type="hidden" name="op" value="download2">
  <input type="hidden" name="id" value="4nol199yw7ze">
  <input type="hidden" name="rand" value="swdsr2raotze6yeneuysj54ep37g4mtsbo6o773ra4">
  <input type="hidden" name="referer" value="https://www.kingmods.net/en/fs25/mods/62939/feliz">
  <input type="hidden" name="method_free" value="">
  <input type="hidden" name="method_premium" value="">
  <button id="downloadbtn" class="btn btn-primary downloadbtn">Create download link</button>
</form>
</body></html>"#;

    /// The same form with no check widget — the shape a
    /// host that lets a client through presents.
    const HOST_PAGE_PLAIN: &str = r#"<!DOCTYPE html>
<html><head><title>Download FS25_Feliz.zip</title></head><body>
<form name="F1" method="POST" action="">
  <input type="hidden" name="op" value="download2">
  <input type="hidden" name="id" value="4nol199yw7ze">
  <input type="hidden" name="rand" value="firstrand111">
  <input type="hidden" name="referer" value="https://www.kingmods.net/en/fs25/mods/62939/feliz">
  <input type="hidden" name="method_free" value="">
  <input type="hidden" name="method_premium" value="">
  <button id="downloadbtn" class="btn btn-primary downloadbtn">Create download link</button>
</form>
</body></html>"#;

    /// What ShareMods answers a tokenless step POST with:
    /// the same form again (a fresh rand), the Turnstile
    /// widget, and its verdict in the page text.
    const HOST_PAGE_WRONG_CAPTCHA: &str = r#"<!DOCTYPE html>
<html><head><title>Download FS25 Feliz zip</title></head><body>
<div class="download__filename-content">
  <strong>FS25_Feliz.zip</strong> <span>(1.2 GB)</span> Wrong captcha
</div>
<div class="cf-turnstile" data-sitekey="0x4AAAAAADFrOJ1ncwtEU1yA"></div>
<form name="F1" method="POST" action="">
  <input type="hidden" name="op" value="download2">
  <input type="hidden" name="id" value="4nol199yw7ze">
  <input type="hidden" name="rand" value="secondrand222">
  <input type="hidden" name="referer" value="https://www.kingmods.net/en/fs25/mods/62939/feliz">
  <input type="hidden" name="method_free" value="">
  <input type="hidden" name="method_premium" value="">
  <button id="downloadbtn" class="btn btn-primary downloadbtn">Start Download</button>
</form>
</body></html>"#;

    fn field<'a>(form: &'a HostForm, name: &str) -> Option<&'a str> {
        form.fields
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    #[test]
    fn host_form_comes_out_of_the_live_sharemods_shape() {
        let form = parse_host_form(HOST_PAGE_CHECKED).expect("the F1 form parses");
        // An empty action posts back to the page itself.
        assert_eq!(form.action, "");
        assert_eq!(field(&form, "op"), Some("download2"));
        assert_eq!(field(&form, "id"), Some("4nol199yw7ze"));
        assert_eq!(
            field(&form, "rand"),
            Some("swdsr2raotze6yeneuysj54ep37g4mtsbo6o773ra4")
        );
        // The host echoes the KingMods referer into the
        // form — the chain starts there, exactly as the
        // flow sends it.
        assert_eq!(
            field(&form, "referer"),
            Some("https://www.kingmods.net/en/fs25/mods/62939/feliz")
        );
        assert_eq!(field(&form, "method_free"), Some(""));
        // And the page's check widget is seen for what it is.
        assert!(demands_browser_check(HOST_PAGE_CHECKED));
        assert!(!demands_browser_check(HOST_PAGE_PLAIN));
        // A clean landing page carries no complaint.
        assert_eq!(host_error_banner(HOST_PAGE_CHECKED), None);
    }

    #[test]
    fn host_banners_are_read_verbatim() {
        assert_eq!(
            host_error_banner(HOST_PAGE_WRONG_CAPTCHA).as_deref(),
            Some("Wrong captcha")
        );
        assert_eq!(
            host_error_banner("<p>Wrong IP address</p>").as_deref(),
            Some("Wrong IP address")
        );
        assert_eq!(
            host_error_banner("<p>File not found</p>").as_deref(),
            Some("File not found")
        );
        assert_eq!(host_error_banner(HOST_PAGE_PLAIN), None);
    }

    #[test]
    fn the_op_form_wins_over_other_forms() {
        // A page with a search box beside the download
        // form: the download form is the one with an op.
        let html = format!(
            r#"<form method="GET" action="/search"><input name="q" value="x"></form>{}"#,
            HOST_PAGE_PLAIN
        );
        let form = parse_host_form(&html).expect("a form parses");
        assert_eq!(field(&form, "op"), Some("download2"));
    }

    #[test]
    fn form_actions_resolve_against_the_page() {
        assert_eq!(
            resolve_action("https://host.example/a/b.zip.html", ""),
            "https://host.example/a/b.zip.html"
        );
        assert_eq!(
            resolve_action("https://host.example/a/b.zip.html", "/dl/step"),
            "https://host.example/dl/step"
        );
        assert_eq!(
            resolve_action("https://host.example/a/b.zip.html", "step2"),
            "https://host.example/a/step2"
        );
    }

    /// A scripted loopback host: answers requests in
    /// script order, whatever they are — the flow tests'
    /// stage set. Each entry is (status line, content
    /// type, body).
    fn stub_script(
        script: Vec<(&'static str, &'static str, Vec<u8>)>,
    ) -> String {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let queue = std::sync::Arc::new(std::sync::Mutex::new(
            script
                .into_iter()
                .collect::<std::collections::VecDeque<_>>(),
        ));
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let mut buf: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 8192];
                let header_end = loop {
                    match stream.read(&mut tmp) {
                        Ok(0) | Err(_) => break None,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            if let Some(pos) = buf
                                .windows(4)
                                .position(|w| w == b"\r\n\r\n")
                            {
                                break Some(pos + 4);
                            }
                        }
                    }
                };
                let Some(hend) = header_end else { continue };
                let head = String::from_utf8_lossy(&buf[..hend]).to_string();
                let clen = head
                    .lines()
                    .find_map(|l| {
                        l.to_lowercase()
                            .strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                let mut have = buf.len() - hend;
                while have < clen {
                    match stream.read(&mut tmp) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            have += n;
                            buf.extend_from_slice(&tmp[..n]);
                        }
                    }
                }
                let (status, ctype, body) = queue
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or((
                        "500 Internal Server Error",
                        "text/html",
                        b"stub script exhausted".to_vec(),
                    ));
                let resp = format!(
                    "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    status,
                    ctype,
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(&body);
            }
        });
        format!("http://{}", addr)
    }

    fn temp_dest(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "playground-km-{}-{}.zip",
            tag,
            std::process::id()
        ))
    }

    /// The payload the scripted hosts serve as "the file":
    /// zip magic and a deterministic run of bytes.
    fn payload() -> Vec<u8> {
        let mut v = b"PK\x03\x04".to_vec();
        v.extend((0..5000u32).map(|i| (i % 251) as u8));
        v
    }

    #[tokio::test]
    async fn the_step_flow_lands_the_file() {
        // GET the landing page, POST to the second step,
        // POST again — and the third answer is the file.
        let step2 = HOST_PAGE_PLAIN.replace("firstrand111", "secondrand222");
        let base = stub_script(vec![
            ("200 OK", "text/html", HOST_PAGE_PLAIN.as_bytes().to_vec()),
            ("200 OK", "text/html", step2.into_bytes()),
            (
                "200 OK",
                "application/octet-stream",
                payload(),
            ),
        ]);
        let url = format!("{}/4nol199yw7ze/FS25_Feliz.zip.html", base);
        let dest = temp_dest("flow");
        let src = KingModsSource::new();
        let mut seen: Vec<(u64, Option<u64>)> = Vec::new();
        let n = src
            .download_to_reporting(
                &url,
                "https://www.kingmods.net/en/fs25/mods/62939/feliz",
                &dest,
                &mut |d, t| seen.push((d, t)),
            )
            .await
            .expect("the flow should complete");
        assert_eq!(n, payload().len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), payload());
        // Progress ended at completion with the total the
        // host stated.
        assert_eq!(seen.last(), Some(&(n, Some(n))));
        let _ = std::fs::remove_file(&dest);
    }

    #[tokio::test]
    async fn a_direct_file_link_needs_no_steps() {
        // A KingMods button pointing straight at a file
        // (the GitHub-asset shape): one request, streamed.
        let base = stub_script(vec![(
            "200 OK",
            "application/octet-stream",
            payload(),
        )]);
        let url = format!("{}/releases/FS25_Feliz.zip", base);
        let dest = temp_dest("direct");
        let n = KingModsSource::new()
            .download_to(&url, "https://www.kingmods.net/en/x", &dest)
            .await
            .expect("a direct file should download");
        assert_eq!(n, payload().len() as u64);
        assert_eq!(std::fs::read(&dest).unwrap(), payload());
        let _ = std::fs::remove_file(&dest);
    }

    #[tokio::test]
    async fn the_browser_check_wall_is_named_not_refused() {
        // The live ShareMods sequence: the checked landing
        // page, then the same form with its verdict.
        let base = stub_script(vec![
            ("200 OK", "text/html", HOST_PAGE_CHECKED.as_bytes().to_vec()),
            (
                "200 OK",
                "text/html",
                HOST_PAGE_WRONG_CAPTCHA.as_bytes().to_vec(),
            ),
        ]);
        let url = format!("{}/4nol199yw7ze/FS25_Feliz.zip.html", base);
        let dest = temp_dest("checked");
        let err = KingModsSource::new()
            .download_to(
                &url,
                "https://www.kingmods.net/en/fs25/mods/62939/feliz",
                &dest,
            )
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("browser check") && msg.contains("Turnstile"),
            "the wall must be named, got: {}",
            msg
        );
        assert!(!dest.exists(), "no file may land on a failed flow");
    }

    #[tokio::test]
    async fn a_403_from_the_host_is_a_throttle_error() {
        let base = stub_script(vec![("403 Forbidden", "text/html", Vec::new())]);
        let url = format!("{}/4nol199yw7ze/FS25_Feliz.zip.html", base);
        let dest = temp_dest("throttled");
        let err = KingModsSource::new()
            .download_to(&url, "https://www.kingmods.net/en/x", &dest)
            .await
            .unwrap_err();
        assert_eq!(crate::sources::throttle::throttle_status(&err), Some(403));
    }
}


