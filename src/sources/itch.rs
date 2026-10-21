use crate::sources::{ModSearchResult, ModSource};
use anyhow::{Context, Result};
use std::path::PathBuf;

/// itch.io update source — the fourth source.
///
/// A mod is linked by its itch project page URL (stored
/// in the catalog-other store, keyed by file stem). itch
/// offers no unauthenticated version API for this: the
/// wharf "latest" endpoint needs a channel name the page
/// doesn't publish, and the general API wants a key. What
/// the project page itself carries, server-rendered, is
/// the upload list — each upload's exact file name in a
/// `<strong class="name" title="...">` — and that is what
/// this leg reads:
///
/// - An upload file name that IS the library's file name
///   (or plainly the same name once a version suffix and
///   separator noise are discounted) anchors the page to
///   this mod. A version encoded in that name
///   ("..._1.2.0.0.zip") is the file's own version.
/// - Some creators keep the project's description set to
///   the bare words "Version 1.2.0.3" — an explicit
///   version field, carried in the page's meta
///   description. When the whole description is just
///   that, it counts, and it outranks a filename version.
/// - Zip uploads that tie to a DIFFERENT mod entirely
///   make the result a mismatch: the page no longer
///   hosts this file, and no version is taken from it.
/// - Uploads itch can't name (externally hosted files
///   carry a display name, not a file name) and uploads
///   with no version anywhere leave the version
///   undetermined — the page was asked, it just doesn't
///   say. That is an honest "unknown", never a guess.
///
/// Like the other legs, a page whose upload list is gone
/// entirely fails loudly — the layout changed.
pub struct ItchSource;

impl ItchSource {
    pub fn new() -> Self {
        Self
    }

    fn client(&self) -> Result<reqwest::Client> {
        Ok(reqwest::Client::builder().user_agent(UA).build()?)
    }

    /// The linked page's current info, or None when the
    /// page is gone (404). `expected_file_name` is the
    /// library's exact file name — the identity the
    /// page's uploads are checked against.
    pub async fn fetch_latest(
        &self,
        url: &str,
        expected_file_name: &str,
    ) -> Result<Option<ItchInfo>> {
        let resp = self
            .client()?
            .get(url)
            .send()
            .await
            .with_context(|| format!("Could not reach itch page {}", url))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        super::throttle::check_status("itch.io", resp.status())?;
        let html = resp
            .error_for_status()
            .with_context(|| format!("itch page failed: {}", url))?
            .text()
            .await
            .context("itch sent an unreadable page")?;
        match parse_page(&html, expected_file_name) {
            Some(info) => Ok(Some(info)),
            None => anyhow::bail!(
                "The itch page {} didn't parse — its layout may have changed.",
                url
            ),
        }
    }
}

const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0 Safari/537.36";

/// What an itch page reports about its mod.
#[derive(Debug, Clone, PartialEq)]
pub struct ItchInfo {
    /// The mod's current version, from the explicit
    /// description field or the matched upload's file
    /// name. None when the page doesn't state one.
    pub version: Option<String>,
    /// The upload file name that anchored the match.
    /// None when the page's uploads carry no file name
    /// for this mod (external/display-named uploads).
    pub file_name: Option<String>,
    /// True when the page's zip uploads exist but none
    /// ties to this mod — the page's files belong to
    /// something else, and nothing on it may be judged.
    pub mismatch: bool,
}

/// The page's upload names (the `title` of each upload's
/// `<strong class="name">`), in page order. Externally
/// hosted uploads appear here too, with their display
/// name — callers tell files from labels by the .zip.
fn upload_names(html: &str) -> Vec<String> {
    let tag_re = regex::Regex::new(r"<strong[^>]*>").unwrap();
    let title_re = regex::Regex::new(r#"title="([^"]+)""#).unwrap();
    let mut names = Vec::new();
    // One segment per upload block; the name sits near
    // the top of its block, well before any later
    // content (comments carry their own <strong>s).
    for seg in html.split("<div class=\"upload\">").skip(1) {
        let head = &seg[..seg.len().min(1500)];
        if let Some(tag) = tag_re.find(head) {
            let tag = tag.as_str();
            if tag.contains("class=\"name\"") {
                if let Some(t) = title_re.captures(tag) {
                    names.push(t[1].to_string());
                }
            }
        }
    }
    names
}

/// The explicit version field: a page description that
/// is NOTHING but a version statement ("Version
/// 1.2.0.3") — the creator's own current-version label,
/// published in the meta description / og:description.
/// A real description that merely mentions a version
/// among other words is prose, not a field, and doesn't
/// count.
fn explicit_version(html: &str) -> Option<String> {
    let meta_re = regex::Regex::new(r"<meta\s[^>]*>").unwrap();
    let content_re = regex::Regex::new(r#"content="([^"]*)""#).unwrap();
    let version_re =
        regex::Regex::new(r"(?i)^\s*(?:version|v)\s*:?\s*(\d+(?:\.\d+)+)\s*$").unwrap();
    for m in meta_re.find_iter(html) {
        let tag = m.as_str();
        if tag.contains("og:description")
            || tag.contains("name=\"description\"")
            || tag.contains("name=\"twitter:description\"")
        {
            if let Some(c) = content_re.captures(tag) {
                if let Some(v) = version_re.captures(c[1].trim()) {
                    return Some(v[1].to_string());
                }
            }
        }
    }
    None
}

/// A version encoded in a file name stem: dotted numbers
/// behind a separator (or the name's start), optionally
/// v-prefixed — "_1.2.0.0", "-v1.2", " 1.0.0". Glued
/// digits ("FS25", "Mod2.5") deliberately don't count:
/// too many mod names carry numbers that aren't
/// versions. With several candidates, the one with the
/// most components wins; ties go to the last — versions
/// trail names.
fn version_in_stem(stem: &str) -> Option<(String, usize, usize)> {
    let re = regex::Regex::new(r"(?i)(?:^|[^0-9a-z.])v?(\d+\.\d+(?:\.\d+)*)").unwrap();
    let mut best: Option<(String, usize, usize)> = None;
    for c in re.captures_iter(stem) {
        let whole = c.get(0).unwrap();
        let ver = c[1].to_string();
        let parts = ver.matches('.').count();
        let better = match &best {
            None => true,
            Some((bv, _, _)) => parts >= bv.matches('.').count(),
        };
        if better {
            best = Some((ver, whole.start(), whole.end()));
        }
    }
    best
}

/// The comparison key for "is this the same mod's
/// name": lowercase alphanumerics only, version
/// discounted. Load-order prefixes the user adds
/// ("FS25_0_...") survive as extra leading characters,
/// which the containment test below absorbs.
fn name_key(file_name: &str, strip_version: bool) -> String {
    let mut stem = file_name
        .strip_suffix(".zip")
        .or_else(|| file_name.strip_suffix(".ZIP"))
        .unwrap_or(file_name)
        .to_string();
    if strip_version {
        if let Some((_, start, end)) = version_in_stem(&stem) {
            stem.replace_range(start..end, "");
        }
    }
    stem.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Does upload `name` tie to the expected file? An
/// exact name match is identity itself. Otherwise the
/// squashed keys must be equal, or one must contain
/// the other with the shorter at least 5 characters —
/// "thbetterwinch" inside "fs250thbetterwinch" is the
/// same mod wearing a load-order prefix; "pack" inside
/// anything proves nothing.
fn ties_to(name: &str, expected_file_name: &str) -> bool {
    if name == expected_file_name {
        return true;
    }
    let a = name_key(name, true);
    let b = name_key(expected_file_name, false);
    if a.is_empty() || b.is_empty() {
        return false;
    }
    if a == b {
        return true;
    }
    let (short, long) = if a.len() <= b.len() {
        (&a, &b)
    } else {
        (&b, &a)
    };
    short.len() >= 5 && long.contains(short.as_str())
}

/// Parse an itch project page for the mod whose exact
/// file name is `expected_file_name`. None only when
/// the page has no upload list at all — its layout is
/// gone, which the caller reports as an error rather
/// than an empty answer.
pub fn parse_page(html: &str, expected_file_name: &str) -> Option<ItchInfo> {
    if !html.contains("upload_list") {
        return None;
    }
    let names = upload_names(html);
    let zips: Vec<&String> = names
        .iter()
        .filter(|n| n.to_lowercase().ends_with(".zip"))
        .collect();
    let explicit = explicit_version(html);

    if zips.is_empty() {
        // No file-named uploads: nothing contradicts the
        // link, and only the explicit field can speak.
        return Some(ItchInfo {
            version: explicit,
            file_name: None,
            mismatch: false,
        });
    }

    let matched: Vec<&&String> = zips
        .iter()
        .filter(|n| ties_to(n, expected_file_name))
        .collect();
    if matched.is_empty() {
        // The page's files belong to a different mod.
        return Some(ItchInfo {
            version: None,
            file_name: None,
            mismatch: true,
        });
    }

    // The anchor file: an exact name match first, else
    // the first tying upload.
    let anchor = matched
        .iter()
        .find(|n| ***n == expected_file_name)
        .copied()
        .unwrap_or(matched[0]);
    // The filename version: the anchor's own, else the
    // highest among the tying uploads (they are all
    // this mod's files).
    let mut file_version = version_in_stem(
        anchor
            .strip_suffix(".zip")
            .or_else(|| anchor.strip_suffix(".ZIP"))
            .unwrap_or(anchor),
    )
    .map(|(v, _, _)| v);
    if file_version.is_none() {
        for n in &matched {
            let stem = n
                .strip_suffix(".zip")
                .or_else(|| n.strip_suffix(".ZIP"))
                .unwrap_or(n);
            if let Some((v, _, _)) = version_in_stem(stem) {
                let newer = match &file_version {
                    None => true,
                    Some(cur) => crate::sources::github::is_newer(&v, cur),
                };
                if newer {
                    file_version = Some(v);
                }
            }
        }
    }

    Some(ItchInfo {
        // The explicit field outranks the file's name.
        version: explicit.or(file_version),
        file_name: Some(anchor.to_string()),
        mismatch: false,
    })
}

// The original synchronous trait stays for shape; the
// real work goes through the async fetch above.
impl ModSource for ItchSource {
    fn name(&self) -> &str {
        "itch.io"
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
    async fn a_403_from_itch_is_a_throttle_error() {
        let src = ItchSource::new();
        let err = src
            .fetch_latest(&stub_status("403 Forbidden"), "FS25_Mod.zip")
            .await
            .unwrap_err();
        assert_eq!(crate::sources::throttle::throttle_status(&err), Some(403));
    }

    fn page(uploads: &[&str], description: Option<&str>) -> String {
        let mut items = String::new();
        for (i, name) in uploads.iter().enumerate() {
            items.push_str(&format!(
                r#"<div class="upload"><a href="javascript:void(0);" data-upload_id="{}" class="button download_btn">Download</a><div class="info_column"><div class="upload_name"><strong class="name" title="{}">{}</strong> <span class="file_size"><span>2.3 MB</span></span></div></div></div>"#,
                1000 + i,
                name,
                name
            ));
        }
        let meta = match description {
            Some(d) => format!(
                r#"<meta content="{}" property="og:description"/><meta content="{}" name="description"/>"#,
                d, d
            ),
            None => String::new(),
        };
        format!(
            r#"<html><head>{}</head><body><div class="uploads"><div id="upload_list_8525046" class="upload_list_widget base_widget">{}</div></div></body></html>"#,
            meta, items
        )
    }

    #[test]
    fn exact_upload_with_no_version_anywhere() {
        // The DrivewayPack shape: the upload IS the
        // library's file, and neither it nor the page
        // states a version — asked, undetermined.
        let html = page(
            &["ACH25_DrivewayPack.zip"],
            Some("Driveways for your farm builds."),
        );
        let info = parse_page(&html, "ACH25_DrivewayPack.zip").unwrap();
        assert_eq!(info.file_name.as_deref(), Some("ACH25_DrivewayPack.zip"));
        assert_eq!(info.version, None);
        assert!(!info.mismatch);
    }

    #[test]
    fn version_comes_from_the_upload_file_name() {
        let html = page(&["FS25_SomeMod_1.4.0.0.zip"], None);
        let info = parse_page(&html, "FS25_SomeMod.zip").unwrap();
        assert_eq!(info.version.as_deref(), Some("1.4.0.0"));
        assert_eq!(
            info.file_name.as_deref(),
            Some("FS25_SomeMod_1.4.0.0.zip")
        );
        assert!(!info.mismatch);
    }

    #[test]
    fn load_order_prefix_still_ties() {
        // The user's copy wears a load-order prefix the
        // upload doesn't: same mod, version still read.
        let html = page(&["TH_BetterWinch_1.2.0.3.zip"], None);
        let info = parse_page(&html, "FS25_0_THBetterWinch.zip").unwrap();
        assert_eq!(info.version.as_deref(), Some("1.2.0.3"));
        assert!(!info.mismatch);
    }

    #[test]
    fn uploads_of_a_different_mod_are_a_mismatch() {
        let html = page(&["TotallyDifferentTractor.zip"], None);
        let info = parse_page(&html, "FS25_SomeMod.zip").unwrap();
        assert!(info.mismatch);
        assert_eq!(info.version, None);
        assert_eq!(info.file_name, None);
    }

    #[test]
    fn explicit_version_field_speaks_for_external_uploads() {
        // The Better Winch shape: the upload is external
        // (a display name, no file), and the description
        // is the bare version statement.
        let html = page(&["Better Winch Collection"], Some("Version 1.2.0.3"));
        let info = parse_page(&html, "FS25_0_THBetterWinch.zip").unwrap();
        assert_eq!(info.version.as_deref(), Some("1.2.0.3"));
        assert_eq!(info.file_name, None);
        assert!(!info.mismatch);
    }

    #[test]
    fn explicit_version_outranks_the_file_name() {
        let html = page(&["FS25_SomeMod_1.4.0.0.zip"], Some("Version 1.5.0.0"));
        let info = parse_page(&html, "FS25_SomeMod.zip").unwrap();
        assert_eq!(info.version.as_deref(), Some("1.5.0.0"));
    }

    #[test]
    fn prose_mentioning_a_version_is_not_a_field() {
        let html = page(
            &["FS25_SomeMod.zip"],
            Some("Now updated! Version 2.0 brings new sheds and fixes."),
        );
        let info = parse_page(&html, "FS25_SomeMod.zip").unwrap();
        assert_eq!(info.version, None);
        assert!(!info.mismatch);
    }

    #[test]
    fn page_without_an_upload_list_does_not_parse() {
        assert!(parse_page("<html><body>gone</body></html>", "X.zip").is_none());
    }
}

