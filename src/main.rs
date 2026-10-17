// Release builds on Windows use the GUI subsystem, so launching the
// finished .exe does not leave a console window behind. Debug builds keep
// the console for diagnostics and CLI work.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

mod mod_manager;
mod scanner;
mod sources;
mod config;
mod ui;

use config::Config;
use mod_manager::ModManager;
use std::env;

fn main() -> anyhow::Result<()> {
    // Setup logging
    tracing_subscriber::fmt::init();

    // Load configuration
    let config = Config::load()?;

    // Check if we should use CLI mode
    let args: Vec<String> = env::args().collect();
    if args.len() > 1 && args[1] == "cli" {
        let mut mod_manager = ModManager::new(config)?;
        return run_cli(&mut mod_manager, &args[2..]);
    }

    // Start GUI — unscanned: the app loads the library in a background
    // task so the window opens immediately.
    let mod_manager = ModManager::new_unscanned(config)?;
    ui::run(mod_manager)
}

fn format_mod_line(m: &mod_manager::Mod) -> String {
    let short_id = &m.id[..m.id.len().min(8)];
    match &m.author {
        Some(author) => format!(
            "{} v{} by {} ({:?}) [{}]",
            m.name, m.version, author, m.state, short_id
        ),
        None => format!("{} v{} ({:?}) [{}]", m.name, m.version, m.state, short_id),
    }
}

fn run_cli(mod_manager: &mut ModManager, args: &[String]) -> anyhow::Result<()> {
    match args.get(0).map(|s| s.as_str()) {
        Some("scan") => {
            mod_manager.scan_all()?;
            let mods = mod_manager.get_mods();
            println!("Found {} mods:", mods.len());
            for m in mods {
                println!("- {}", format_mod_line(m));
            }
        }
        Some("list") => {
            let mods = mod_manager.get_mods();
            println!("Mods in archive: {}", mod_manager.get_mods_by_state(mod_manager::ModState::Archived).len());
            println!("Mods active: {}", mod_manager.get_mods_by_state(mod_manager::ModState::Active).len());
            for m in mods {
                println!("- {}", format_mod_line(m));
            }
        }
        Some("search") => {
            if let Some(query) = args.get(1) {
                let results = mod_manager.search_mods(query);
                println!("Found {} matching mods:", results.len());
                for m in results {
                    println!("- {}", format_mod_line(m));
                }
            }
        }
        Some("move") => {
            if let Some(query) = args.get(1) {
                if let Some(target) = args.get(2) {
                    // Accept a full ID, the short ID shown by scan/list, or
                    // the mod's name.
                    let found = mod_manager
                        .get_mods()
                        .into_iter()
                        .find(|m| {
                            m.id == *query
                                || m.id.starts_with(query.as_str())
                                || m.name.eq_ignore_ascii_case(query)
                        })
                        .map(|m| m.id.clone());
                    match found {
                        Some(id) => {
                            let target_path = std::path::PathBuf::from(target);
                            mod_manager.move_between_directories(&id, &target_path)?;
                            println!("Moved mod to {:?}", target_path);
                        }
                        None => println!("No mod matching '{}'", query),
                    }
                }
            }
        }
        Some("icons") => {
            for m in mod_manager.get_mods() {
                match mod_manager::icons::load_icon(&m.path, m.icon_filename.as_deref()) {
                    Some(mod_manager::icons::LoadedIcon::Rgba { width, height, .. }) => {
                        println!("- {}: DDS icon decoded ({}x{})", m.name, width, height)
                    }
                    Some(mod_manager::icons::LoadedIcon::Encoded(bytes)) => {
                        println!("- {}: image icon ({} bytes)", m.name, bytes.len())
                    }
                    None => println!("- {}: no icon found", m.name),
                }
            }
        }
        Some("conflicts") => {
            let mods = mod_manager.get_mods();
            let mods_vec: Vec<mod_manager::Mod> = mods.into_iter().cloned().collect();
            let conflicts = mod_manager::conflict::ConflictDetector::detect_conflicts(&mods_vec);
            println!("Found {} conflicts:", conflicts.len());
            for c in conflicts {
                println!("- {} vs {}: {:?}", c.mod1, c.mod2, c.conflict_type);
            }
        }
        Some("categories") => {
            let mut counts: std::collections::HashMap<&str, usize> =
                std::collections::HashMap::new();
            for m in mod_manager.get_mods() {
                println!("- {}: {}", m.name, m.categories.join(", "));
                for c in &m.categories {
                    *counts.entry(c.as_str()).or_insert(0) += 1;
                }
            }
            let mut summary: Vec<(&str, usize)> = counts.into_iter().collect();
            summary.sort_by(|a, b| b.1.cmp(&a.1));
            println!("Category counts:");
            for (cat, n) in summary {
                println!("  {}: {}", cat, n);
            }
        }
        Some("saves") => {
            let slots = mod_manager.save_slots();
            if slots.is_empty() {
                println!("No save slots found (is the active folder a game's mods folder?).");
            }
            for s in &slots {
                let loadout = mod_manager.save_loadout(&s.dir_name);
                println!(
                    "- Slot {} ({}): map {}, saved {}, played {}, money {} — loadout: {} mods",
                    s.slot,
                    s.dir_name,
                    s.map.as_deref().unwrap_or("?"),
                    s.saved_at.as_deref().unwrap_or("?"),
                    s.play_time_display().as_deref().unwrap_or("?"),
                    s.money
                        .map(|m| format!("${}", m))
                        .unwrap_or_else(|| "?".to_string()),
                    if loadout.is_empty() {
                        "none saved".to_string()
                    } else {
                        format!("{} mods", loadout.len())
                    },
                );
            }
        }
        Some("audit") => {
            if let Some(root) = args.get(1) {
                audit_tree(mod_manager, std::path::Path::new(root));
            } else {
                println!("Usage: cli audit <folder>");
                println!("Lists the mods inside <folder> (searched recursively) that");
                println!("are NOT in the library, so a duplicate folder tree can be");
                println!("checked before anything in it is deleted.");
            }
        }
        Some("link-modhub") => {
            match (args.get(1), args.get(2)) {
                (Some(stem), Some(reference)) => {
                    match crate::sources::mod_hub::normalize_modhub(reference) {
                        Some(mod_id) => {
                            mod_manager.set_modhub_link(&stem.to_lowercase(), Some(mod_id.clone()))?;
                            println!("Linked {} -> ModHub mod {}", stem.to_lowercase(), mod_id);
                        }
                        None => println!("Not a ModHub reference: {} (use the mod's page URL or its numeric id)", reference),
                    }
                }
                _ => println!("Usage: cli link-modhub <mod file stem> <ModHub page URL or mod id>"),
            }
        }
        Some("link") => {
            match (args.get(1), args.get(2)) {
                (Some(stem), Some(repo)) => {
                    match crate::sources::github::normalize_repo(repo) {
                        Some(repo) => {
                            mod_manager.set_mod_link(&stem.to_lowercase(), Some(repo.clone()))?;
                            println!("Linked {} -> {}", stem.to_lowercase(), repo);
                        }
                        None => println!("Not a repo reference: {} (use owner/repo or the repo URL)", repo),
                    }
                }
                _ => println!("Usage: cli link <mod file stem> <owner/repo>"),
            }
        }
        Some("updates") => {
            let links = mod_manager.mod_links();
            let mh_links = mod_manager.modhub_links();
            if links.is_empty() && mh_links.is_empty() {
                // Even with nothing linked, the catalog may
                // know some of the library — check it alone.
                let runtime = tokio::runtime::Runtime::new()?;
                let found_any = runtime.block_on(async {
                    catalog_updates_pass(&mod_manager, true).await
                });
                if !found_any {
                    println!("No mods are linked, and the catalog doesn't know any mod in the library. Link one with: cli link <stem> <owner/repo> or cli link-modhub <stem> <ModHub URL or id>");
                }
            } else {
                let (base, token) = mod_manager.github_endpoint();
                let runtime = tokio::runtime::Runtime::new()?;
                runtime.block_on(async {
                    let src = crate::sources::github::GithubSource::new(token);
                    for (stem, repo) in &links {
                        let current = mod_manager
                            .get_mods()
                            .into_iter()
                            .find(|m| mod_manager::ModManager::mod_stem(&m.path) == *stem)
                            .map(|m| m.version.clone());
                        match current {
                            Some(current) => match src.latest_release(&base, repo).await {
                                Ok(Some(rel)) => {
                                    if crate::sources::github::is_newer(&rel.version, &current) {
                                        println!("- {} ({}): v{} installed, v{} available", stem, repo, current, rel.version);
                                    } else {
                                        println!("- {} ({}): up to date (v{})", stem, repo, current);
                                    }
                                }
                                Ok(None) => println!("- {} ({}): no releases on GitHub", stem, repo),
                                Err(e) => println!("- {} ({}): check failed: {}", stem, repo, e),
                            },
                            None => println!("- {} ({}): linked, but no mod with that file name is in the library", stem, repo),
                        }
                    }
                    let mh = crate::sources::mod_hub::ModHubSource::new();
                    for (stem, mod_id) in &mh_links {
                        let current = mod_manager
                            .get_mods()
                            .into_iter()
                            .find(|m| mod_manager::ModManager::mod_stem(&m.path) == *stem)
                            .map(|m| m.version.clone());
                        match current {
                            Some(current) => match mh.fetch_info(mod_id).await {
                                Ok(Some(info)) => {
                                    if crate::sources::github::is_newer(&info.version, &current) {
                                        println!("- {} (ModHub {}): v{} installed, v{} available", stem, mod_id, current, info.version);
                                    } else {
                                        println!("- {} (ModHub {}): up to date (v{})", stem, mod_id, current);
                                    }
                                }
                                Ok(None) => println!("- {} (ModHub {}): the mod's page is gone", stem, mod_id),
                                Err(e) => println!("- {} (ModHub {}): check failed: {}", stem, mod_id, e),
                            },
                            None => println!("- {} (ModHub {}): linked, but no mod with that file name is in the library", stem, mod_id),
                        }
                    }
                    // Catalog pass: unlinked mods the
                    // published catalog knows get an update
                    // answer locally, no ModHub traffic.
                    catalog_updates_pass(&mod_manager, false).await;
                });
            }
        }
        Some("update") => {
            let Some(stem) = args.get(1).map(|s| s.to_lowercase()) else {
                println!("Usage: cli update <mod file stem>");
                return Ok(());
            };
            let repo = mod_manager.mod_link(&stem).map(|s| s.to_string());
            let mut mh_id = mod_manager.modhub_link(&stem).map(|s| s.to_string());
            if repo.is_none() && mh_id.is_none() {
                // Not linked — but the catalog may know it,
                // in which case it updates like a linked mod.
                let rt = tokio::runtime::Runtime::new()?;
                mh_id = rt.block_on(async {
                    match crate::sources::catalog::ensure(&mod_manager.catalog_url()).await {
                        Some(cat) => crate::sources::catalog::by_stem(&cat)
                            .get(&stem)
                            .map(|e| {
                                println!(
                                    "(Not linked, but the catalog knows it — ModHub {}.)",
                                    e.modhub_id
                                );
                                e.modhub_id.clone()
                            }),
                        None => None,
                    }
                });
            }
            if repo.is_none() && mh_id.is_none() {
                println!("{} is not linked to GitHub or ModHub, and the catalog doesn't know it. Link it first: cli link {} <owner/repo> or cli link-modhub {} <ModHub URL or id>", stem, stem, stem);
                return Ok(());
            };
            let Some(m) = mod_manager
                .get_mods()
                .into_iter()
                .find(|m| mod_manager::ModManager::mod_stem(&m.path) == stem)
                .cloned()
            else {
                println!("No mod named {} in the library.", stem);
                return Ok(());
            };
            let (base, token) = mod_manager.github_endpoint();
            let runtime = tokio::runtime::Runtime::new()?;
            let dest = m
                .path
                .parent()
                .map(|p| p.join(format!(".playground-dl-{}.zip", stem)))
                .unwrap_or_default();
            let outcome: Result<String, String> = runtime.block_on(async {
                if let Some(repo) = &repo {
                    let src = crate::sources::github::GithubSource::new(token);
                    let release = src
                        .latest_release(&base, repo)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "The repo has no releases.".to_string())?;
                    if !crate::sources::github::is_newer(&release.version, &m.version) {
                        return Err(format!("Already up to date (v{}).", m.version));
                    }
                    let asset = crate::sources::github::pick_asset(&release, &stem)
                        .ok_or_else(|| format!("Release {} has no .zip asset to install.", release.tag))?;
                    let url = asset.browser_download_url.clone();
                    src.download_to(&url, &dest)
                        .await
                        .map_err(|e| e.to_string())?;
                    return Ok(release.version.clone());
                }
                let mod_id = mh_id.clone().unwrap_or_default();
                let src = crate::sources::mod_hub::ModHubSource::new();
                let info = src
                    .fetch_info(&mod_id)
                    .await
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "The mod's ModHub page is gone.".to_string())?;
                if !crate::sources::github::is_newer(&info.version, &m.version) {
                    return Err(format!("Already up to date (v{}).", m.version));
                }
                let page = crate::sources::mod_hub::page_url(&mod_id);
                src.download_to(&info.download_url, &page, &dest)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(info.version.clone())
            });
            match outcome {
                Ok(version) => match mod_manager.replace_mod_file(&m.id, &dest) {
                    Ok(change) => println!("Updated {}: {} (release v{}).", stem, change, version),
                    Err(e) => println!("Install failed: {}", e),
                },
                Err(e) => println!("{}", e),
            }
            let _ = std::fs::remove_file(&dest);
        }
        Some("detect-modhub") => {
            // Snapshot the zip mods that have no ModHub link yet,
            // one job per unique file stem.
            let mut jobs: Vec<(String, String, String)> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for m in mod_manager.get_mods() {
                let stem = mod_manager::ModManager::mod_stem(&m.path);
                if mod_manager.modhub_link(&stem).is_some() || !seen.insert(stem.clone()) {
                    continue;
                }
                let Some(file_name) = m
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                else {
                    continue;
                };
                if !file_name.to_lowercase().ends_with(".zip") {
                    continue; // folder mods are not ModHub downloads
                }
                jobs.push((stem, file_name, m.name.clone()));
            }
            let limit: Option<usize> = args.get(1).and_then(|s| s.parse().ok());
            if let Some(n) = limit {
                jobs.truncate(n);
            }
            // Catalog pre-pass: stems the published catalog
            // knows link instantly; only the rest go through
            // live searching.
            let runtime = tokio::runtime::Runtime::new()?;
            let catalog_index = runtime.block_on(async {
                match crate::sources::catalog::ensure(&mod_manager.catalog_url()).await {
                    Some(cat) => crate::sources::catalog::by_stem(&cat),
                    None => std::collections::HashMap::new(),
                }
            });
            if !catalog_index.is_empty() {
                let mut hits: Vec<String> = Vec::new();
                jobs.retain(|(stem, _, _)| {
                    if catalog_index.contains_key(stem) {
                        hits.push(stem.clone());
                        false
                    } else {
                        true
                    }
                });
                for stem in &hits {
                    let id = catalog_index[stem].modhub_id.clone();
                    mod_manager.set_modhub_link(stem, Some(id.clone()))?;
                    println!("- {} -> ModHub {} (catalog)", stem, id);
                }
                if !hits.is_empty() {
                    println!(
                        "{} of the mods linked instantly from the catalog.",
                        hits.len()
                    );
                }
            }
            if jobs.is_empty() {
                if catalog_index.is_empty() {
                    println!("Nothing to detect — every .zip mod already has a ModHub link (or the library is empty).");
                } else {
                    println!("Done: every remaining mod was linked from the catalog.");
                }
            } else {
                println!(
                    "Auto-detecting ModHub links for {} mods (search, then exact filename verification)…",
                    jobs.len()
                );
                let src = crate::sources::mod_hub::ModHubSource::new();
                let (mut found, mut missed) = (0usize, 0usize);
                for (stem, file_name, title) in &jobs {
                    match runtime.block_on(src.find_mod_id(file_name, title, 4)) {
                        Ok(Some(id)) => {
                            found += 1;
                            mod_manager.set_modhub_link(stem, Some(id.clone()))?;
                            println!("- {} -> ModHub {}", stem, id);
                        }
                        Ok(None) => {
                            missed += 1;
                            println!("- {} -> not found on ModHub", stem);
                        }
                        Err(e) => {
                            missed += 1;
                            println!("- {} -> detection failed: {}", stem, e);
                        }
                    }
                }
                println!(
                    "Done: {} linked, {} not found. Links are saved as they are found, so re-running only retries the unlinked.",
                    found, missed
                );
            }
        }
        Some("catalog-fetch") => {
            // Download (or refresh) the published catalog into
            // the local cache the app reads from.
            let url = mod_manager.catalog_url();
            let runtime = tokio::runtime::Runtime::new()?;
            if url.trim().is_empty() {
                match crate::sources::catalog::load_cache() {
                    Some(cat) => println!(
                        "Catalog fetching is disabled (empty catalog_url); the local cache holds {} mods.",
                        cat.mods.len()
                    ),
                    None => println!(
                        "Catalog fetching is disabled (empty catalog_url) and no cache exists yet."
                    ),
                }
            } else {
                match runtime.block_on(crate::sources::catalog::fetch(&url)) {
                    Ok(cat) => println!(
                        "Catalog downloaded: {} mods, generated {} (unix). Cached for the app.",
                        cat.mods.len(),
                        cat.generated_at
                    ),
                    Err(e) => println!("Catalog download failed: {}", e),
                }
            }
        }
        Some("catalog-build") => {
            // The catalog indexer: walk ModHub's FS25 listings
            // (or take explicit ids), fetch each mod's page with
            // the shared parser, and merge the records into a
            // static catalog JSON. Merging makes runs resumable
            // and incremental — ids already catalogued are
            // skipped unless --refresh is given.
            let Some(out) = args.get(1) else {
                println!("Usage: cli catalog-build <catalog.json> [--pages N] [--max N] [--ids 123,456] [--stale DAYS] [--refresh]");
                return Ok(());
            };
            let out_path = std::path::Path::new(out);
            let mut pages: Option<u32> = None;
            let mut max: Option<usize> = None;
            let mut ids_arg: Option<Vec<String>> = None;
            let mut refresh = false;
            let mut stale_days: u64 = 7;
            let mut it = args.iter().skip(2).peekable();
            while let Some(a) = it.next() {
                match a.as_str() {
                    "--pages" => pages = it.next().and_then(|v| v.parse().ok()),
                    "--max" => max = it.next().and_then(|v| v.parse().ok()),
                    "--stale" => {
                        if let Some(v) = it.next().and_then(|v| v.parse().ok()) {
                            stale_days = v;
                        }
                    }
                    "--ids" => {
                        ids_arg = it.next().map(|v| {
                            v.split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect()
                        })
                    }
                    "--refresh" => refresh = true,
                    _ => {}
                }
            }
            let mut catalog = if out_path.exists() {
                crate::sources::mod_hub::Catalog::load(out_path)
                    .unwrap_or_else(|_| crate::sources::mod_hub::Catalog::empty())
            } else {
                crate::sources::mod_hub::Catalog::empty()
            };
            let known: std::collections::HashSet<String> = catalog
                .mods
                .iter()
                .map(|e| e.modhub_id.clone())
                .collect();
            let src = crate::sources::mod_hub::ModHubSource::new();
            let runtime = tokio::runtime::Runtime::new()?;
            // Decide which ids to fetch.
            let now_secs = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let mut todo_ids: Vec<String> = Vec::new();
            if let Some(list) = ids_arg {
                for id in list {
                    if refresh || !known.contains(&id) {
                        todo_ids.push(id);
                    }
                }
            } else {
                let page_cap = pages.unwrap_or(u32::MAX);
                let mut page = 0u32;
                let mut seen = std::collections::HashSet::new();
                while page < page_cap {
                    match runtime.block_on(src.listing_ids(page)) {
                        Ok(ids) => {
                            if ids.is_empty() {
                                break;
                            }
                            let mut new_to_catalog = 0usize;
                            for id in ids {
                                if seen.insert(id.clone())
                                    && !known.contains(&id)
                                {
                                    new_to_catalog += 1;
                                    todo_ids.push(id);
                                }
                            }
                            println!(
                                "Listing page {}: {} new to the catalog ({} to fetch so far)",
                                page,
                                new_to_catalog,
                                todo_ids.len()
                            );
                            // Listings are newest-first: once a
                            // whole page is already catalogued,
                            // deeper pages only get older.
                            if new_to_catalog == 0 && !refresh {
                                break;
                            }
                        }
                        Err(e) => {
                            println!("Listing page {} failed: {}", page, e);
                            break;
                        }
                    }
                    page += 1;
                }
                // Queue stale records for re-verification so
                // version bumps surface without a full re-walk;
                // --refresh queues everything instead.
                let stale_before = now_secs.saturating_sub(stale_days * 86_400);
                let mut queued: std::collections::HashSet<String> =
                    todo_ids.iter().cloned().collect();
                for e in &catalog.mods {
                    let due = refresh || e.checked_at < stale_before;
                    if due && queued.insert(e.modhub_id.clone()) {
                        todo_ids.push(e.modhub_id.clone());
                    }
                }
            }
            if let Some(m) = max {
                todo_ids.truncate(m);
            }
            if todo_ids.is_empty() {
                println!("Nothing to fetch — the catalog already covers the requested ids.");
            } else {
                println!(
                    "Fetching {} mod pages (four at a time, politely)…",
                    todo_ids.len()
                );
                let mut by_id: std::collections::HashMap<String, crate::sources::mod_hub::CatalogEntry> =
                    catalog
                        .mods
                        .iter()
                        .map(|e| (e.modhub_id.clone(), e.clone()))
                        .collect();
                let (mut done, mut recorded, mut failed) = (0usize, 0usize, 0usize);
                for chunk in todo_ids.chunks(4) {
                    let results = runtime.block_on(futures::future::join_all(
                        chunk.iter().map(|id| src.fetch_info(id)),
                    ));
                    for (id, res) in chunk.iter().zip(results) {
                        match res {
                            Ok(Some(info)) => {
                                let stem = info
                                    .file_name
                                    .strip_suffix(".zip")
                                    .unwrap_or(&info.file_name)
                                    .to_lowercase();
                                by_id.insert(
                                    id.clone(),
                                    crate::sources::mod_hub::CatalogEntry {
                                        stem,
                                        file_name: info.file_name.clone(),
                                        modhub_id: id.clone(),
                                        version: info.version.clone(),
                                        download_url: info.download_url.clone(),
                                        checked_at: now_secs,
                                    },
                                );
                                recorded += 1;
                            }
                            Ok(None) => {
                                failed += 1;
                                println!("- {} : page gone", id);
                            }
                            Err(e) => {
                                failed += 1;
                                println!("- {} : {}", id, e);
                            }
                        }
                    }
                    done += chunk.len();
                    if done % 100 < 4 || done == todo_ids.len() {
                        println!("  …{} of {} fetched", done, todo_ids.len());
                    }
                    runtime.block_on(async {
                        tokio::time::sleep(std::time::Duration::from_millis(80)).await
                    });
                }
                catalog.mods = by_id.into_values().collect();
                catalog.mods.sort_by(|a, b| a.stem.cmp(&b.stem));
                catalog.generated_at = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                catalog.save(out_path)?;
                println!(
                    "Catalog written to {} — {} mods total ({} newly recorded, {} failed this run).",
                    out,
                    catalog.mods.len(),
                    recorded,
                    failed
                );
            }
        }
        Some("help") | _ => {
            print_help();
        }
    }

    Ok(())
}

/// Human-readable byte count for CLI output.
fn fmt_bytes(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < units.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{} {}", n, units[i])
    } else {
        format!("{:.1} {}", v, units[i])
    }
}

/// Recursive folder audit (for John's library consolidation):
/// walk a folder tree, treat every .zip and every folder holding
/// a modDesc.xml as one mod, and diff their file stems against
/// the scanned library. Prints what the tree duplicates (safe to
/// delete once the uniques are rescued) and what exists ONLY in
/// the tree. Read-only — nothing is moved or deleted.
fn audit_tree(mod_manager: &mut ModManager, root: &std::path::Path) {
    use std::collections::HashSet;
    if !root.is_dir() {
        println!("Not a folder: {:?}", root);
        return;
    }
    let library: HashSet<String> = mod_manager
        .get_mods()
        .into_iter()
        .map(|m| mod_stem(&m.path))
        .collect();

    fn mod_stem(path: &std::path::Path) -> String {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        name.strip_suffix(".zip")
            .map(|s| s.to_string())
            .unwrap_or(name)
    }

    fn collect(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if path.join("modDesc.xml").exists() {
                    out.push(path); // a folder mod — do not descend
                } else {
                    collect(&path, out);
                }
            } else if path
                .extension()
                .map_or(false, |e| e.eq_ignore_ascii_case("zip"))
            {
                out.push(path);
            }
        }
    }

    let mut candidates = Vec::new();
    if root.join("modDesc.xml").exists() {
        candidates.push(root.to_path_buf());
    } else {
        collect(root, &mut candidates);
    }
    candidates.sort();

    let mut dup_count = 0usize;
    let mut dup_bytes = 0u64;
    let mut unique: Vec<(String, String, u64, std::path::PathBuf)> = Vec::new();
    for path in &candidates {
        let (label, version, size) = match mod_manager::Mod::new(path.clone()) {
            Ok(m) => (m.name.clone(), m.version.clone(), m.size),
            Err(_) => (
                path.file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                "?".to_string(),
                std::fs::metadata(path).map(|md| md.len()).unwrap_or(0),
            ),
        };
        if library.contains(&mod_stem(path)) {
            dup_count += 1;
            dup_bytes += size;
        } else {
            unique.push((label, version, size, path.clone()));
        }
    }
    let unique_bytes: u64 = unique.iter().map(|u| u.2).sum();

    println!("Audit of {:?}", root);
    println!("Library holds {} mods.", library.len());
    println!("Found {} mods in the tree:", candidates.len());
    println!(
        "- {} already in the library ({}) — duplicates",
        dup_count,
        fmt_bytes(dup_bytes)
    );
    println!(
        "- {} ONLY in this tree ({}) — rescue these before deleting the tree",
        unique.len(),
        fmt_bytes(unique_bytes)
    );
    for (label, version, size, path) in &unique {
        println!(
            "  UNIQUE: {} v{} ({}) at {:?}",
            label,
            version,
            fmt_bytes(*size),
            path
        );
    }
}

/// The CLI updates pass over unlinked mods using the
/// published catalog: any scanned mod the catalog knows gets
/// a local version comparison — no ModHub traffic, nothing
/// about the library leaves the machine. Prints only
/// available updates plus a one-line summary. Returns
/// whether the catalog recognized anything at all.
async fn catalog_updates_pass(mod_manager: &ModManager, standalone: bool) -> bool {
    let Some(cat) = crate::sources::catalog::ensure(&mod_manager.catalog_url()).await else {
        return false;
    };
    let index = crate::sources::catalog::by_stem(&cat);
    let linked: std::collections::HashSet<String> = mod_manager
        .mod_links()
        .into_iter()
        .map(|(s, _)| s)
        .chain(mod_manager.modhub_links().into_iter().map(|(s, _)| s))
        .collect();
    let mut seen = std::collections::HashSet::new();
    let (mut recognized, mut updates) = (0usize, 0usize);
    for m in mod_manager.get_mods() {
        let stem = ModManager::mod_stem(&m.path);
        if linked.contains(&stem) || !seen.insert(stem.clone()) {
            continue;
        }
        let Some(entry) = index.get(&stem) else {
            continue;
        };
        recognized += 1;
        if crate::sources::github::is_newer(&entry.version, &m.version) {
            updates += 1;
            println!(
                "- {} (ModHub {}, via catalog): v{} installed, v{} available",
                stem, entry.modhub_id, m.version, entry.version
            );
        }
    }
    if recognized > 0 {
        println!(
            "Catalog: {} unlinked mods recognized, {} updates available.",
            recognized, updates
        );
    } else if standalone {
        println!("The catalog doesn't know any mod in the library yet.");
    }
    recognized > 0
}

fn print_help() {
    println!("FS Mod Manager - Farming Simulator Mod Manager");
    println!("Usage: fs-mod-manager [command] [args]");
    println!("Commands:");
    println!("  cli scan - Scan all directories for mods");
    println!("  cli list - List all mods");
    println!("  cli search <query> - Search mods");
    println!("  cli move <id|name> <target_dir> - Move mod to directory");
    println!("  cli conflicts - Check for mod conflicts");
    println!("  cli icons - Show icon load status for each mod");
    println!("  cli categories - Show the detected category of each mod");
    println!("  cli saves - List savegame slots and their saved loadouts");
    println!("  cli audit <folder> - List mods in a folder tree that are NOT in the library");
    println!("  cli link <stem> <owner/repo> - Link a mod to its GitHub repo for update checks");
    println!("  cli link-modhub <stem> <url|id> - Link a mod to its GIANTS ModHub page for update checks");
    println!("  cli updates - Check every linked mod (GitHub and ModHub) for updates");
    println!("  cli update <stem> - Download and install the linked mod's latest version");
    println!("  cli detect-modhub [limit] - Auto-link mods to ModHub by verified filename match");
    println!("  cli catalog-fetch - Download/refresh the published ModHub catalog into the local cache");
    println!("  cli catalog-build <catalog.json> [--pages N] [--max N] [--ids a,b] [--refresh] - Build/refresh the static ModHub catalog");
    println!("  cli help - Show this help");
    println!("  (no args) - Launch GUI");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_load() {
        let config = Config::load();
        assert!(config.is_ok());
    }
}