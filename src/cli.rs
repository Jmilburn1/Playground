//! The command-line surface: every `cli` verb, shared by
//! the app binary (`playground cli ...`) and the
//! standalone update program (`update.exe`, src/bin/update.rs),
//! which exposes the network-talking verbs — the fill
//! (`update`) and ModHub detection (`detect-modhub`) —
//! without the GUI. One implementation, two entry points,
//! so the two programs can never drift apart on what the
//! verbs do.

use crate::mod_manager::{self, ModManager};

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

pub fn run_cli(mod_manager: &mut ModManager, args: &[String]) -> anyhow::Result<()> {
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
                println!("No save slots found (is the game mods folder a game's mods folder?).");
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
        Some("db-fill") => {
            // Deprecated alias (owner's naming, 2026-10-19):
            // the fill verb is `update` now — "update" is
            // the call to arms, not a file. The old name
            // keeps working because the owner's scripts and
            // muscle memory still carry it.
            eprintln!("note: 'db-fill' is deprecated — the fill verb is now 'update' (running it).");
            run_fill(mod_manager)?;
        }
        Some("update") if args.get(1).is_none() => {
            // The update: the one swipe that fills the
            // personal update database (scan facts + catalog
            // join + link polls) and writes catalog-home.json
            // beside the other stores. This is the verb the
            // standalone update program runs, and the one the
            // scheduler fires ~3x/day. With a stem argument
            // instead, `update` means the single-mod install
            // below.
            run_fill(mod_manager)?;
        }
        Some("update") => {
            let stem = args
                .get(1)
                .map(|s| s.to_lowercase())
                .expect("the arm above handled the no-argument case");
            let repo = mod_manager.mod_link(&stem).map(|s| s.to_string());
            let mut mh_id = mod_manager.modhub_link(&stem).map(|s| s.to_string());
            if repo.is_none() && mh_id.is_none() {
                // Not linked — but the catalog may know it,
                // in which case it updates like a linked mod.
                let rt = tokio::runtime::Runtime::new()?;
                mh_id = rt.block_on(async {
                    match crate::sources::catalog::ensure(&mod_manager.catalog_url(), mod_manager.cache_ttl_hours()).await {
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
            let limit: Option<usize> = args.get(1).and_then(|s| s.parse().ok());
            let src = crate::sources::mod_hub::ModHubSource::new();
            detect_modhub(mod_manager, limit, &src)?;
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
            //
            // --full is the COMPLETE WALK (John, 2026-10-18):
            // walk every listing page to the very end with no
            // early stop, and fetch the page of EVERY mod the
            // listings show — not just the new ones — so each
            // record ends up carrying all four searched tags
            // (id, download link, filename, Released date)
            // plus its version, and the run can report the
            // complete-list diff: how many mods ModHub has,
            // how many were missing from the catalog, and how
            // many catalogued ids the walk never saw (delisted
            // candidates; their records are kept, not deleted).
            let Some(out) = args.get(1) else {
                println!("Usage: cli catalog-build <catalog.json> [--pages N] [--max N] [--ids 123,456] [--stale DAYS] [--refresh] [--full]");
                return Ok(());
            };
            let out_path = std::path::Path::new(out);
            let mut pages: Option<u32> = None;
            let mut max: Option<usize> = None;
            let mut ids_arg: Option<Vec<String>> = None;
            let mut refresh = false;
            let mut full = false;
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
                    "--full" => full = true,
                    _ => {}
                }
            }
            // Resume from the output file — or, during the
            // rename transition, from its twin when only the
            // twin is on disk.
            let load_path = if out_path.exists() {
                out_path.to_path_buf()
            } else {
                catalog_twin_path(out_path)
                    .filter(|t| t.exists())
                    .unwrap_or_else(|| out_path.to_path_buf())
            };
            let mut catalog = if load_path.exists() {
                crate::sources::mod_hub::Catalog::load(&load_path)
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
            // The complete list, when the listing walk runs:
            // every mod_id ModHub showed, in walk order. The
            // --full report diffs this against the catalog.
            let mut walk_ids: Vec<String> = Vec::new();
            // True only when the walk reached an empty listing
            // page — the natural end — rather than stopping at
            // a failed page or the --pages cap.
            let mut walk_complete = false;
            if let Some(list) = ids_arg {
                for id in list {
                    if full || refresh || !known.contains(&id) {
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
                                walk_complete = true;
                                break;
                            }
                            let mut new_to_catalog = 0usize;
                            for id in ids {
                                if seen.insert(id.clone()) {
                                    walk_ids.push(id.clone());
                                    if !known.contains(&id) {
                                        new_to_catalog += 1;
                                    }
                                    // A normal walk queues only
                                    // ids the catalog lacks. A
                                    // --full walk queues EVERY id
                                    // it sees: every mod page gets
                                    // re-fetched, so every record
                                    // is re-verified and gains its
                                    // Released date.
                                    if full || !known.contains(&id) {
                                        todo_ids.push(id);
                                    }
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
                            // deeper pages only get older. A
                            // --full walk never stops early — the
                            // complete list is the whole point.
                            if new_to_catalog == 0 && !refresh && !full {
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
                if full && !walk_ids.is_empty() {
                    println!(
                        "WARNING (--full): the walk saw {} mods but queued none — the report would be empty. That is not a complete walk.",
                        walk_ids.len()
                    );
                }
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
                                        released: info.released.clone(),
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
                    // Checkpoint every 500 fetched pages: a full
                    // walk runs about an hour, and a run killed
                    // by a dropped connection or a closed laptop
                    // must not lose everything since the start.
                    // The catalog on disk stays a valid merge of
                    // everything fetched so far; re-running the
                    // same command picks up from there.
                    if done % 500 < 4 || done == todo_ids.len() {
                        let mut snapshot: Vec<crate::sources::mod_hub::CatalogEntry> =
                            by_id.values().cloned().collect();
                        snapshot.sort_by(|a, b| a.stem.cmp(&b.stem));
                        let checkpoint = crate::sources::mod_hub::Catalog {
                            format: 1,
                            generated_at: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0),
                            mods: snapshot,
                        };
                        if let Err(e) = save_catalog_dual(&checkpoint, out_path) {
                            println!("  (checkpoint save failed: {})", e);
                        }
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
                save_catalog_dual(&catalog, out_path)?;
                println!(
                    "Catalog written to {} — {} mods total ({} newly recorded, {} failed this run).{}",
                    out,
                    catalog.mods.len(),
                    recorded,
                    failed,
                    if catalog_twin_path(out_path).is_some() {
                        " (Also written under its twin name — the rename dual-publishes catalog.json and catalog-hub.json.)"
                    } else {
                        ""
                    }
                );
                if full {
                    // The complete-list report: what ModHub has,
                    // what the catalog had, and what this walk
                    // left outstanding. Written beside the
                    // catalog as well as printed, so an
                    // unattended run leaves a receipt.
                    let walk_set: std::collections::HashSet<&String> =
                        walk_ids.iter().collect();
                    let new_ids: Vec<&String> = walk_ids
                        .iter()
                        .filter(|id| !known.contains(*id))
                        .collect();
                    let final_released: std::collections::HashMap<&str, &str> =
                        catalog
                            .mods
                            .iter()
                            .map(|e| (e.modhub_id.as_str(), e.released.as_str()))
                            .collect();
                    let still_missing: Vec<&String> = walk_ids
                        .iter()
                        .filter(|id| {
                            final_released
                                .get(id.as_str())
                                .map(|r| r.is_empty())
                                .unwrap_or(true)
                        })
                        .collect();
                    let not_seen: Vec<&String> = known
                        .iter()
                        .filter(|id| !walk_set.contains(id))
                        .collect();
                    let with_date = catalog
                        .mods
                        .iter()
                        .filter(|e| !e.released.is_empty())
                        .count();
                    let mut report = String::new();
                    report.push_str("Playground complete-walk report (--full)\n");
                    report.push_str(&format!("Catalog file: {}\n", out));
                    report.push_str(&format!(
                        "Walk reached the end of the listings: {}\n",
                        if walk_complete { "yes" } else { "NO — the walk stopped early, this list is incomplete" }
                    ));
                    report.push_str(&format!(
                        "Mods on ModHub (ids seen in the walk): {}\n",
                        walk_ids.len()
                    ));
                    report.push_str(&format!(
                        "New to the catalog in this walk: {}\n",
                        new_ids.len()
                    ));
                    report.push_str(&format!(
                        "Catalog records now carrying a Released date: {} of {}\n",
                        with_date,
                        catalog.mods.len()
                    ));
                    report.push_str(&format!(
                        "Walked mods still missing a record/date after the fetch: {}\n",
                        still_missing.len()
                    ));
                    report.push_str(&format!(
                        "Catalogued ids the walk did NOT see (delisted candidates — records kept, not deleted): {}\n",
                        not_seen.len()
                    ));
                    report.push_str(&format!(
                        "New ids: {}\n",
                        new_ids
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                    report.push_str(&format!(
                        "Still-missing ids: {}\n",
                        still_missing
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                    report.push_str(&format!(
                        "Not-seen ids: {}\n",
                        not_seen
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(",")
                    ));
                    print!("\n{}", report);
                    let report_path = out_path
                        .parent()
                        .filter(|p| !p.as_os_str().is_empty())
                        .map(|p| p.join("catalog-full-report.txt"))
                        .unwrap_or_else(|| {
                            std::path::PathBuf::from("catalog-full-report.txt")
                        });
                    match std::fs::write(&report_path, &report) {
                        Ok(_) => println!("Report written to {:?}", report_path),
                        Err(e) => println!("Could not write the report file: {}", e),
                    }
                }
            }
        }
        Some("layout-plan") => {
            // Roadmap #2, Phase 1: the read-only layout
            // engine. Prints the migration plan and saves
            // it as layout-plan.json in the config dir —
            // Phase 2's input contract. Nothing in the
            // library or the game folder is touched.
            let plan = mod_manager.layout_plan();
            print!("{}", crate::mod_manager::layout::render_text(&plan));
            let path = crate::mod_manager::layout::write_plan_json(&plan)?;
            println!("Plan saved to {}", path.display());
        }
        Some("layout-apply") => {
            // Roadmap #2, Phase 2: execute the Library
            // Layout migration — archive sort, staging
            // farm, verification, swap — recorded op by op
            // in layout-state.json. Refuses (rather than
            // improvising) on a cross-volume pair, an open
            // earlier migration, or a saved plan for other
            // folders. The destructive step is printed in
            // plain words before anything is written.
            crate::mod_manager::layout_exec::run_apply(mod_manager)?;
        }
        Some("layout-rollback") => {
            // Roadmap #2, Phase 2: restore the
            // pre-migration world from layout-state.json —
            // audit the farm, swap the backup back, undo
            // the archive sort. The backup itself is only
            // ever consumed by being renamed home; it is
            // never deleted.
            crate::mod_manager::layout_exec::run_rollback()?;
        }
        Some("help") | _ => {
            print_help();
        }
    }

    Ok(())
}


/// The update fill, shared by the `update` verb, its
/// deprecated `db-fill` alias, and the standalone update
/// program. A tripwire abort inside the fill surfaces here
/// as an error — the caller returns it from main, which is
/// what makes the halt a nonzero exit.
fn run_fill(mod_manager: &mut ModManager) -> anyhow::Result<()> {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async { crate::update_db::fill(mod_manager).await })?;
    Ok(())
}

/// What a ModHub detection pass did — returned so callers
/// (and tests) can see the shape of the pass without
/// scraping its printed lines. A pass stopped by the
/// GIANTS tripwire returns an error instead; the counts up
/// to the halt were printed as they happened.
#[derive(Debug, Default, PartialEq)]
pub struct DetectOutcome {
    pub catalog_linked: usize,
    pub searched: usize,
    pub found: usize,
    pub not_found: usize,
    pub skipped_misses: usize,
    pub skipped_other: usize,
    pub skipped_stevie: usize,
}

/// The ModHub detection pass (the `detect-modhub` verb,
/// also run by the standalone update program): reconcile
/// the miss store against catalog-other, link what the
/// published catalog knows, and live-search only the true
/// residue — remembering each outcome (owner's run order,
/// 2026-10-20). The ModHub leg is a parameter so tests can
/// point it at a stub.
///
/// TRIPWIRE (owner, 2026-10-19): a 403/429 from GIANTS
/// halts the pass on the spot — the search loop stops, the
/// pass returns an error (a nonzero exit for scripts and
/// the scheduler), and links/misses already written stand:
/// they are facts, not a database overwrite.
pub fn detect_modhub(
    mod_manager: &mut ModManager,
    limit: Option<usize>,
    src: &crate::sources::mod_hub::ModHubSource,
) -> anyhow::Result<DetectOutcome> {
        // Run order, step 1 (owner, 2026-10-20): read
        // catalog-other and reconcile the miss store
        // against it before anything else — a mod that
        // gained a link there loses its miss record.
        // This runs even when no search follows.
        let pruned = mod_manager.prune_misses();
        if pruned > 0 {
            println!(
                "Forgot {} remembered ModHub miss(es) — those mods have a home in catalog-other now.",
                pruned
            );
        }
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
        if let Some(n) = limit {
            jobs.truncate(n);
        }
        // The published catalog (run order step 2).
        let runtime = tokio::runtime::Runtime::new()?;
        let catalog_index = runtime.block_on(async {
            match crate::sources::catalog::ensure(&mod_manager.catalog_url(), mod_manager.cache_ttl_hours()).await {
                Some(cat) => crate::sources::catalog::by_stem(&cat),
                None => std::collections::HashMap::new(),
            }
        });
        // Classify every job by the run order: Stevie
        // (never searched) -> catalog-other -> the
        // catalog -> remembered miss -> live search.
        // Only the residue — present in NO store — is
        // searched, and its outcome is written down.
        use crate::sources::catalog_miss::Decision;
        let mut hits: Vec<(String, String)> = Vec::new(); // (stem, file_name)
        let mut rest: Vec<(Decision, String, String, String)> = Vec::new();
        for (stem, file_name, title) in jobs {
            let decision = mod_manager.detect_decision(
                &file_name,
                &stem,
                catalog_index.contains_key(&stem),
            );
            match decision {
                Decision::InCatalog => hits.push((stem, file_name)),
                _ => rest.push((decision, stem, file_name, title)),
            }
        }
        // Catalog pre-pass: stems the published catalog
        // knows link instantly, exactly as before.
        for (stem, file_name) in &hits {
            let id = catalog_index[stem].modhub_id.clone();
            mod_manager.set_modhub_link(stem, Some(id.clone()))?;
            mod_manager.forget_miss(file_name, stem);
            println!("- {} -> ModHub {} (catalog)", stem, id);
        }
        if !hits.is_empty() {
            println!(
                "{} of the mods linked instantly from the catalog.",
                hits.len()
            );
        }
        // The rest, in the original order: skips say why
        // no search is spent on them; the true residue
        // goes on to the live search below.
        let mut search: Vec<(String, String, String)> = Vec::new();
        let (mut n_miss, mut n_other, mut n_stevie) = (0usize, 0usize, 0usize);
        for (decision, stem, file_name, title) in rest {
            match decision {
                Decision::KnownMiss => {
                    n_miss += 1;
                    println!(
                        "- {} -> not on ModHub (a previous search found nothing; remembered, not searched again)",
                        stem
                    );
                }
                Decision::InOther => {
                    n_other += 1;
                    println!(
                        "- {} -> has a home in catalog-other, so no ModHub search is spent on it",
                        stem
                    );
                }
                Decision::Stevie => {
                    n_stevie += 1;
                    println!(
                        "- {} -> Stevie mod (manual install only), never searched",
                        stem
                    );
                }
                _ => search.push((stem, file_name, title)),
            }
        }
        if search.is_empty() {
            if !hits.is_empty() {
                println!("Done: every remaining mod was linked from the catalog.");
            } else if n_miss + n_other + n_stevie > 0 {
                println!("Done: nothing needed a live search.");
            } else if catalog_index.is_empty() {
                println!("Nothing to detect — every .zip mod already has a ModHub link (or the library is empty).");
            } else {
                println!("Done: every remaining mod was linked from the catalog.");
            }
        } else {
            println!(
                "Auto-detecting ModHub links for {} mods (search, then exact filename verification)…",
                search.len()
            );
            let (mut found, mut missed) = (0usize, 0usize);
            let mut searched = 0usize;
            let mut halted: Option<u16> = None;
            for (stem, file_name, title) in &search {
                searched += 1;
                match runtime.block_on(src.find_mod_id(file_name, title, 4)) {
                    Ok(Some(id)) => {
                        found += 1;
                        mod_manager.set_modhub_link(stem, Some(id.clone()))?;
                        mod_manager.forget_miss(file_name, stem);
                        println!("- {} -> ModHub {}", stem, id);
                    }
                    Ok(None) => {
                        // The outcome is written down: a
                        // remembered miss, so this search
                        // is never burned again.
                        missed += 1;
                        mod_manager.record_miss(file_name, stem);
                        println!("- {} -> not found on ModHub", stem);
                    }
                    Err(e) => {
                        // THE TRIPWIRE: GIANTS said stop.
                        // The loop ends here — the mods not
                        // yet searched are simply not
                        // searched, and the error below makes
                        // the halt visible to scripts as a
                        // nonzero exit. Links and misses
                        // already written stand.
                        if let Some(status) =
                            crate::sources::throttle::throttle_status(&e)
                        {
                            halted = Some(status);
                            println!(
                                "- {} -> TRIPWIRE: GIANTS ModHub throttled detection (HTTP {}) — the pass halts here",
                                stem, status
                            );
                            break;
                        }
                        // Any other failed search found
                        // nothing out — it is NOT recorded
                        // as a miss; the next run retries it.
                        missed += 1;
                        println!("- {} -> detection failed: {}", stem, e);
                    }
                }
            }
            if n_miss + n_other + n_stevie > 0 {
                println!(
                    "Skipped without a search: {} remembered miss(es), {} homed in catalog-other, {} Stevie (manual-only).",
                    n_miss, n_other, n_stevie
                );
            }
            if let Some(status) = halted {
                println!(
                    "Detection HALTED by the GIANTS tripwire (HTTP {}): {} linked before the halt; the rest were not searched.",
                    status, found
                );
                return Err(anyhow::Error::new(crate::sources::throttle::Throttled {
                    source: "GIANTS ModHub",
                    status,
                })
                .context(
                    "Detection halted by the GIANTS tripwire — links and misses already saved stand",
                ));
            }
            println!(
                "Done: {} linked, {} not found. Links are saved as they are found, so re-running only retries the unlinked.",
                found, missed
            );
            return Ok(DetectOutcome {
                catalog_linked: hits.len(),
                searched,
                found,
                not_found: missed,
                skipped_misses: n_miss,
                skipped_other: n_other,
                skipped_stevie: n_stevie,
            });
        }
        if n_miss + n_other + n_stevie > 0 {
            println!(
                "Skipped without a search: {} remembered miss(es), {} homed in catalog-other, {} Stevie (manual-only).",
                n_miss, n_other, n_stevie
            );
        }
        Ok(DetectOutcome {
            catalog_linked: hits.len(),
            searched: 0,
            found: 0,
            not_found: 0,
            skipped_misses: n_miss,
            skipped_other: n_other,
            skipped_stevie: n_stevie,
        })
}

/// The twin path of a published catalog file: the
/// owner's rename (2026-10-19) dual-publishes the ModHub
/// catalog under both names during the transition —
/// `catalog.json` <-> `catalog-hub.json`. Any other file
/// name has no twin.
fn catalog_twin_path(path: &std::path::Path) -> Option<std::path::PathBuf> {
    match path.file_name().and_then(|n| n.to_str()) {
        Some("catalog.json") => Some(path.with_file_name("catalog-hub.json")),
        Some("catalog-hub.json") => Some(path.with_file_name("catalog.json")),
        _ => None,
    }
}

/// Save a built catalog under its path AND its twin name,
/// so a publish always writes both names with identical
/// content (checkpoints included — a reader who finds the
/// walk mid-flight still gets a consistent catalog under
/// either name).
fn save_catalog_dual(
    cat: &crate::sources::mod_hub::Catalog,
    path: &std::path::Path,
) -> anyhow::Result<()> {
    cat.save(path)?;
    if let Some(twin) = catalog_twin_path(path) {
        cat.save(&twin)?;
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
    let Some(cat) = crate::sources::catalog::ensure(&mod_manager.catalog_url(), mod_manager.cache_ttl_hours()).await else {
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
    println!("Playground - Farming Simulator Mod Manager");
    println!("Usage: playground [command] [args]");
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
    println!("  cli detect-modhub [limit] - Auto-link mods to ModHub by verified filename match");
    println!("  cli catalog-fetch - Download/refresh the published ModHub catalog into the local cache");
    println!("  cli catalog-build <catalog.json> [--pages N] [--max N] [--ids a,b] [--refresh] [--full] - Build/refresh the static ModHub catalog (--full: complete walk — every listing page, every mod page, adds Released dates, writes catalog-full-report.txt). Writes catalog-hub.json alongside catalog.json (dual-publish)");
    println!("  cli update - Run the update fill: fill the personal update database (catalog-home.json): scan + catalog join + link polls, in one swipe");
    println!("  cli update <stem> - Download and install the linked mod's latest version");
    println!("  cli db-fill - Deprecated alias of 'cli update' (the fill)");
    println!("  cli layout-plan - Print the Library Layout migration plan (read-only) and save it as layout-plan.json");
    println!("  cli layout-apply - Execute the Library Layout migration (sort + projection farm + swap; backup kept, rollback with layout-rollback)");
    println!("  cli layout-rollback - Undo a Library Layout migration from its state record (restores the original game folder and the flat library)");
    println!("  cli help - Show this help");
    println!("  (no args) - Launch GUI");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn test_config_load() {
        let config = Config::load();
        assert!(config.is_ok());
    }

    #[test]
    fn catalog_twin_names_swap() {
        assert_eq!(
            catalog_twin_path(std::path::Path::new("/x/catalog.json")),
            Some(std::path::PathBuf::from("/x/catalog-hub.json"))
        );
        assert_eq!(
            catalog_twin_path(std::path::Path::new("/x/catalog-hub.json")),
            Some(std::path::PathBuf::from("/x/catalog.json"))
        );
        assert_eq!(catalog_twin_path(std::path::Path::new("/x/other.json")), None);
    }

    #[test]
    fn dual_save_writes_both_names_with_identical_content() {
        let dir = std::env::temp_dir().join(format!(
            "mm-dual-publish-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let cat = crate::sources::mod_hub::Catalog::empty();
        save_catalog_dual(&cat, &dir.join("catalog.json")).unwrap();
        let a = std::fs::read(dir.join("catalog.json")).unwrap();
        let b = std::fs::read(dir.join("catalog-hub.json")).unwrap();
        assert_eq!(a, b, "dual-publish means identical bytes under both names");
        std::fs::remove_dir_all(&dir).ok();
    }

    /// A loopback stub that counts its requests and answers
    /// every one 403 — the detect tripwire's injection seam.
    fn counting_throttle_stub() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let hits = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let hits2 = hits.clone();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                hits2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(
                    b"HTTP/1.1 403 Forbidden\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });
        (format!("http://{}", addr), hits)
    }

    fn write_zip_mod(path: &std::path::Path, title: &str) {
        use std::io::Write as _;
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file("modDesc.xml", opts).unwrap();
        zip.write_all(
            format!(
                "<modDesc><author>T</author><title><en>{}</en></title><version>1.0.0.0</version></modDesc>",
                title
            )
            .as_bytes(),
        )
        .unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn detect_halts_on_the_giants_tripwire_and_searches_no_further() {
        use crate::config::{GameDirectories, ModSources, Settings};
        let base = std::env::temp_dir().join(format!(
            "mm-detect-tripwire-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&base);
        let archive = base.join("archive");
        let active = base.join("active");
        std::fs::create_dir_all(&archive).unwrap();
        std::fs::create_dir_all(&active).unwrap();
        write_zip_mod(&archive.join("FS25_Tripwire_One.zip"), "One");
        write_zip_mod(&archive.join("FS25_Tripwire_Two.zip"), "Two");
        let config = Config {
            game_directories: GameDirectories {
                fs17: None,
                fs19: None,
                fs22: None,
                fs25: None,
            },
            mod_sources: ModSources {
                mod_hub: vec![],
                github: vec![],
                kingmods: vec![],
            },
            settings: Settings {
                archive_directory: archive,
                active_directory: active,
                check_conflicts: true,
                parallel_scans: 2,
                cache_ttl_hours: 24,
                favorites: vec![],
                save_loadouts: std::collections::HashMap::new(),
                game_directory: None,
                mod_links: std::collections::HashMap::new(),
                modhub_links: std::collections::HashMap::new(),
                catalog_url: String::new(),
            },
        };
        let mut mm = ModManager::new(config).unwrap();
        let (base_url, hits) = counting_throttle_stub();
        let src = crate::sources::mod_hub::ModHubSource::with_base(&base_url);
        let err = detect_modhub(&mut mm, None, &src).unwrap_err();
        assert_eq!(
            crate::sources::throttle::throttle_status(&err),
            Some(403),
            "the halt must carry the throttle signal: {}",
            err
        );
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the first throttled search stops the pass — the second mod is never searched"
        );
        std::fs::remove_dir_all(&base).ok();
    }
}
