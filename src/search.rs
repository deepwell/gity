use crate::git;
use crate::logger::Logger;
use git2::{ObjectType, Oid, Repository};
use gtk::{gio, glib, prelude::*};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

// Search state: (search_query, matching_indices, current_index)
pub type SearchState = Arc<Mutex<(String, Vec<u32>, usize)>>;

#[derive(Debug, Clone, PartialEq, Eq)]
struct OidIndexKey {
    repo_path: PathBuf,
    branch_ref: String,
}

#[derive(Debug)]
struct OidIndexCacheState {
    key: Option<OidIndexKey>,
    generation: u64,
    building: bool,
    oids: Option<Arc<Vec<Oid>>>,
}

#[derive(Debug)]
struct OidIndexCache {
    state: Mutex<OidIndexCacheState>,
    ready: Condvar,
}

impl OidIndexCache {
    fn new() -> Self {
        Self {
            state: Mutex::new(OidIndexCacheState {
                key: None,
                generation: 0,
                building: false,
                oids: None,
            }),
            ready: Condvar::new(),
        }
    }

    fn build_oids_in_revwalk_order(
        key: &OidIndexKey,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<Vec<Oid>, git2::Error> {
        let repo = Repository::open(&key.repo_path)?;
        let opts = git::CommitQueryOptions::for_branch(&key.branch_ref);

        let mut revwalk = repo.revwalk()?;
        // Match `CommitWalker::new` defaults for `for_branch` (no explicit sorting, not reversed).
        revwalk.set_sorting(git2::Sort::NONE)?;

        for spec in &opts.revspecs {
            if spec.starts_with('^') {
                let obj = repo.revparse_single(&spec[1..])?;
                revwalk.hide(obj.id())?;
                continue;
            }
            let revspec = repo.revparse(spec)?;
            if revspec.mode().contains(git2::RevparseMode::SINGLE) {
                revwalk.push(revspec.from().unwrap().id())?;
            } else {
                let from = revspec.from().unwrap().id();
                let to = revspec.to().unwrap().id();
                revwalk.push(to)?;
                if revspec.mode().contains(git2::RevparseMode::MERGE_BASE) {
                    let base = repo.merge_base(from, to)?;
                    let o = repo.find_object(base, Some(ObjectType::Commit))?;
                    revwalk.push(o.id())?;
                }
                revwalk.hide(from)?;
            }
        }
        if opts.revspecs.is_empty() {
            revwalk.push_head()?;
        }

        let mut out: Vec<Oid> = Vec::new();
        for oid_res in revwalk {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err(git2::Error::from_str("Cancelled"));
            }
            out.push(oid_res?);
        }
        Ok(out)
    }

    fn get_or_build(
        &self,
        repo_path: &PathBuf,
        branch_ref: &str,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<Arc<Vec<Oid>>, String> {
        let wanted = OidIndexKey {
            repo_path: repo_path.clone(),
            branch_ref: branch_ref.to_string(),
        };

        loop {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Err("Cancelled".to_string());
            }

            let mut st = self.state.lock().unwrap();

            // Invalidate cache if repo/branch changed.
            if st.key.as_ref() != Some(&wanted) {
                st.key = Some(wanted.clone());
                st.oids = None;
                st.building = false;
                st.generation = st.generation.wrapping_add(1);
            }

            if let Some(oids) = st.oids.clone() {
                return Ok(oids);
            }

            if st.building {
                let (st2, _) = self
                    .ready
                    .wait_timeout(st, Duration::from_millis(50))
                    .unwrap();
                st = st2;
                continue;
            }

            // Start building outside the lock.
            st.building = true;
            let build_gen = st.generation;
            let key = st.key.clone().unwrap();
            drop(st);

            let started_at = std::time::Instant::now();
            Logger::info(&format!(
                "Building commit OID index for {}@{} ...",
                key.repo_path.display(),
                key.branch_ref
            ));

            let built = Self::build_oids_in_revwalk_order(&key, cancel);

            let elapsed_ms = started_at.elapsed().as_millis();
            let mut st = self.state.lock().unwrap();
            if st.generation == build_gen && st.key.as_ref() == Some(&key) {
                match built {
                    Ok(oids) => {
                        Logger::info(&format!(
                            "Built commit OID index: {} commits - {}ms",
                            oids.len(),
                            elapsed_ms
                        ));
                        st.oids = Some(Arc::new(oids));
                    }
                    Err(e) => {
                        let msg = e.message().to_string();
                        if msg.contains("Cancelled") {
                            Logger::info(&format!(
                                "Commit OID index build cancelled - {}ms",
                                elapsed_ms
                            ));
                            st.building = false;
                            self.ready.notify_all();
                            return Err("Cancelled".to_string());
                        }
                        st.building = false;
                        self.ready.notify_all();
                        return Err(format!("Error building commit OID index: {}", e));
                    }
                }
            }

            st.building = false;
            self.ready.notify_all();

            if let Some(oids) = st.oids.clone() {
                return Ok(oids);
            }
            // If the key/generation changed mid-build, loop and try again.
        }
    }
}

// Search result that can be sent through a channel
#[derive(Clone)]
pub struct SearchResult {
    pub query: String,
    /// Global commit indices (revwalk order) matching the query.
    ///
    /// These indices correspond 1:1 with the commit list order shown in the UI.
    pub matching_indices: Vec<u32>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct SearchHandler {
    pub state: SearchState,
    oid_cache: Arc<OidIndexCache>,
}

impl SearchHandler {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new((String::new(), Vec::new(), 0))),
            oid_cache: Arc::new(OidIndexCache::new()),
        }
    }

    /// Find matching commit indices (revwalk order) from git repository.
    pub fn find_matching_indices_in_repo(
        &self,
        path: &PathBuf,
        branch_ref: &str,
        query: &str,
    ) -> Result<Vec<u32>, String> {
        self.find_matching_indices_in_repo_cancelable(path, branch_ref, query, None)
    }

    pub fn find_matching_indices_in_repo_cancelable(
        &self,
        path: &PathBuf,
        branch_ref: &str,
        query: &str,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Vec<u32>, String> {
        let query_lower = query.to_lowercase();
        if query_lower.is_empty() {
            return Ok(Vec::new());
        }

        if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err("Cancelled".to_string());
        }

        let oids = self
            .oid_cache
            .get_or_build(path, branch_ref, cancel.as_ref())?;

        // If the query looks like a plausible SHA prefix (7–40 hex chars), try SHA matching
        // first. If we find *any* SHA matches, return only those indices and skip text search.
        let should_try_sha = {
            let len = query.len();
            (7..=40).contains(&len) && query.chars().all(|c| c.is_ascii_hexdigit())
        };
        if should_try_sha {
            let started_at = std::time::Instant::now();
            let sha_matches = find_sha_prefix_matches(&oids, &query_lower, cancel.as_ref())?;
            if !sha_matches.is_empty() {
                Logger::info(&format!(
                    "SHA search hit: prefix \"{}\" - {} matches - {}ms",
                    query,
                    sha_matches.len(),
                    started_at.elapsed().as_millis()
                ));
                return Ok(sha_matches);
            }
        }

        // Full-text search fallback (full commit message, case-insensitive).
        let started_at = std::time::Instant::now();
        let matches = find_text_matches_parallel(path, oids, query, &query_lower, cancel.as_ref())?;
        Logger::info(&format!(
            "Text search completed: \"{}\" - {} matches - {}ms",
            query,
            matches.len(),
            started_at.elapsed().as_millis()
        ));
        Ok(matches)
    }

    pub fn perform_search_async_cancelable(
        &self,
        path: PathBuf,
        branch_name: Option<String>,
        query: String,
        cancel: Option<Arc<AtomicBool>>,
    ) -> std::sync::mpsc::Receiver<SearchResult> {
        let (tx, rx) = std::sync::mpsc::channel();

        let handler = self.clone();
        std::thread::spawn(move || {
            let query_text = query.clone();
            let start_time = std::time::Instant::now();
            Logger::info(&format!("Search query started: \"{}\"", query_text));

            if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                Logger::info(&format!("Search query cancelled: \"{}\"", query_text));
                return;
            }

            let result = match handler.find_matching_indices_in_repo_cancelable(
                &path,
                branch_name.as_deref().unwrap_or("HEAD"),
                &query,
                cancel.clone(),
            ) {
                Ok(matching_indices) => {
                    let elapsed_ms = start_time.elapsed().as_millis();
                    Logger::info(&format!(
                        "Search query completed: \"{}\" - found {} matches - {}ms",
                        query_text,
                        matching_indices.len(),
                        elapsed_ms
                    ));
                    SearchResult {
                        query: query.clone(),
                        matching_indices,
                        error: None,
                    }
                }
                Err(e) => {
                    if e == "Cancelled" {
                        Logger::info(&format!("Search query cancelled: \"{}\"", query_text));
                        return;
                    }
                    let elapsed_ms = start_time.elapsed().as_millis();
                    Logger::info(&format!(
                        "Search query completed with error: \"{}\" - {}ms",
                        query_text, elapsed_ms
                    ));
                    SearchResult {
                        query: query.clone(),
                        matching_indices: Vec::new(),
                        error: Some(e),
                    }
                }
            };

            if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                Logger::info(&format!("Search query cancelled: \"{}\"", query_text));
                return;
            }

            let _ = tx.send(result);
        });

        rx
    }

    /// Scroll to a specific item index in the scrolled window
    pub fn scroll_to_item(
        scrolled_window: &gtk::ScrolledWindow,
        selection_model: &gtk::SingleSelection,
        item_index: u32,
    ) {
        let scrolled_window1 = scrolled_window.clone();
        let scrolled_window2 = scrolled_window.clone();
        let selection_model1 = selection_model.clone();
        let selection_model2 = selection_model.clone();
        let item_index_for_scroll = item_index;

        // First timeout: initial scroll attempt
        glib::timeout_add_local(std::time::Duration::from_millis(100), move || {
            let vadjustment = scrolled_window1.vadjustment();
            let upper = vadjustment.upper();
            let page_size = vadjustment.page_size();

            let total_items = selection_model1.n_items();
            if total_items > 0 {
                let item_ratio = item_index_for_scroll as f64 / total_items as f64;
                let target_position = item_ratio * upper;
                let target_scroll = (target_position - page_size / 2.0)
                    .max(0.0)
                    .min(upper - page_size);
                vadjustment.set_value(target_scroll);
            }
            glib::ControlFlow::Break
        });

        // Second timeout: fine-tune scroll position
        glib::timeout_add_local(std::time::Duration::from_millis(300), move || {
            let vadjustment = scrolled_window2.vadjustment();
            let upper = vadjustment.upper();
            let page_size = vadjustment.page_size();
            let current_value = vadjustment.value();

            let total_items = selection_model2.n_items();
            if total_items > 0 {
                let item_ratio = item_index_for_scroll as f64 / total_items as f64;
                let target_position = item_ratio * upper;
                let target_scroll = (target_position - page_size / 2.0)
                    .max(0.0)
                    .min(upper - page_size);

                // Only adjust if significantly off
                if (target_scroll - current_value).abs() > 10.0 {
                    vadjustment.set_value(target_scroll);
                }
            }
            glib::ControlFlow::Break
        });
    }

    /// Process search results and update UI asynchronously
    pub fn process_search_result_async(
        &self,
        result: SearchResult,
        store: gio::ListStore,
        selection_model: gtk::SingleSelection,
        scrolled_window: gtk::ScrolledWindow,
        callback: impl FnOnce(Option<usize>) + 'static,
    ) {
        // Handle error case
        if let Some(ref error) = result.error {
            Logger::error(error);
            callback(None);
            return;
        }

        if result.matching_indices.is_empty() {
            callback(Some(0));
            return;
        }

        let matching_indices = result.matching_indices;
        let match_count = matching_indices.len();
        let query = result.query;

        // Update search state - always start with first match
        {
            let mut state = self.state.lock().unwrap();
            *state = (query, matching_indices.clone(), 0);
        }

        // Select and scroll to the first matching commit.
        //
        // IMPORTANT: callers must ensure the target index is already present in the store.
        if let Some(&match_index) = matching_indices.get(0) {
            if match_index < store.n_items() {
                selection_model.select_item(match_index, true);
                Self::scroll_to_item(&scrolled_window, &selection_model, match_index);
            }
        }

        callback(Some(match_count));
    }

    /// Compute the next match index (global revwalk order), updating internal state.
    pub fn compute_next_match_index(
        &self,
        query: String,
        path: &PathBuf,
        branch_ref: &str,
    ) -> Option<u32> {
        let (matching_indices, had_cached, previous_pos) = {
            let state = self.state.lock().unwrap();
            if !state.0.is_empty() && state.0 == query && !state.1.is_empty() {
                (state.1.clone(), true, state.2)
            } else {
                (Vec::new(), false, 0usize)
            }
        };

        let matching_indices = if had_cached {
            matching_indices
        } else {
            let indices = self
                .find_matching_indices_in_repo(path, branch_ref, &query)
                .ok()?;
            if indices.is_empty() {
                return None;
            }
            indices
        };

        let next_pos = if had_cached {
            (previous_pos + 1) % matching_indices.len()
        } else {
            0
        };

        let target = *matching_indices.get(next_pos)?;
        {
            let mut state = self.state.lock().unwrap();
            *state = (query, matching_indices, next_pos);
        }
        Some(target)
    }

    /// Compute the previous match index (global revwalk order), updating internal state.
    pub fn compute_previous_match_index(
        &self,
        query: String,
        path: &PathBuf,
        branch_ref: &str,
    ) -> Option<u32> {
        let (matching_indices, had_cached, previous_pos) = {
            let state = self.state.lock().unwrap();
            if !state.0.is_empty() && state.0 == query && !state.1.is_empty() {
                (state.1.clone(), true, state.2)
            } else {
                (Vec::new(), false, 0usize)
            }
        };

        let matching_indices = if had_cached {
            matching_indices
        } else {
            let indices = self
                .find_matching_indices_in_repo(path, branch_ref, &query)
                .ok()?;
            if indices.is_empty() {
                return None;
            }
            indices
        };

        let prev_pos = if had_cached {
            if previous_pos == 0 {
                matching_indices.len() - 1
            } else {
                previous_pos - 1
            }
        } else {
            matching_indices.len() - 1
        };

        let target = *matching_indices.get(prev_pos)?;
        {
            let mut state = self.state.lock().unwrap();
            *state = (query, matching_indices, prev_pos);
        }
        Some(target)
    }
}

fn parse_hex_prefix(prefix_lower: &str) -> Result<(Vec<u8>, Option<u8>), String> {
    // prefix is already validated as ascii-hexdigit-only.
    let bytes = prefix_lower.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0usize;
    while i + 1 < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16).unwrap() as u8;
        let lo = (bytes[i + 1] as char).to_digit(16).unwrap() as u8;
        out.push((hi << 4) | lo);
        i += 2;
    }
    let odd_nibble = if i < bytes.len() {
        Some((bytes[i] as char).to_digit(16).unwrap() as u8)
    } else {
        None
    };
    Ok((out, odd_nibble))
}

fn find_sha_prefix_matches(
    oids: &[Oid],
    prefix_lower: &str,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<Vec<u32>, String> {
    let (full_bytes, odd_nibble) = parse_hex_prefix(prefix_lower)?;
    let mut out: Vec<u32> = Vec::new();
    for (i, oid) in oids.iter().enumerate() {
        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
            return Err("Cancelled".to_string());
        }
        let oid_bytes = oid.as_bytes();
        if !full_bytes.is_empty() && oid_bytes[..full_bytes.len()] != full_bytes[..] {
            continue;
        }
        if let Some(nib) = odd_nibble {
            let b = oid_bytes[full_bytes.len()];
            if (b >> 4) != nib {
                continue;
            }
        }
        out.push(i as u32);
    }
    Ok(out)
}

fn contains_ascii_case_insensitive(haystack: &[u8], needle_lower: &[u8]) -> bool {
    if needle_lower.is_empty() {
        return true;
    }
    if haystack.len() < needle_lower.len() {
        return false;
    }

    let first = needle_lower[0];
    let first_upper = first.to_ascii_uppercase();

    'outer: for i in 0..=(haystack.len() - needle_lower.len()) {
        let b0 = haystack[i];
        if b0 != first && b0 != first_upper {
            continue;
        }
        for j in 1..needle_lower.len() {
            if haystack[i + j].to_ascii_lowercase() != needle_lower[j] {
                continue 'outer;
            }
        }
        return true;
    }
    false
}

fn find_text_matches_parallel(
    repo_path: &PathBuf,
    oids: Arc<Vec<Oid>>,
    query: &str,
    query_lower: &str,
    cancel: Option<&Arc<AtomicBool>>,
) -> Result<Vec<u32>, String> {
    let total = oids.len();
    if total == 0 {
        return Ok(Vec::new());
    }

    let available = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let min_chunk = 50_000usize;
    let max_workers = (total + min_chunk - 1) / min_chunk;
    let workers = available.max(1).min(max_workers.max(1));

    let needle_lower_ascii: Option<Vec<u8>> = if query.is_ascii() {
        Some(
            query
                .as_bytes()
                .iter()
                .map(|b| b.to_ascii_lowercase())
                .collect(),
        )
    } else {
        None
    };

    let (tx, rx) = std::sync::mpsc::channel::<Result<Vec<u32>, String>>();
    let chunk_size = (total + workers - 1) / workers;

    for worker_idx in 0..workers {
        let start = worker_idx * chunk_size;
        if start >= total {
            break;
        }
        let end = ((worker_idx + 1) * chunk_size).min(total);
        let tx = tx.clone();
        let repo_path = repo_path.clone();
        let oids = oids.clone();
        let cancel = cancel.cloned();
        let query_lower = query_lower.to_string();
        let needle_lower_ascii = needle_lower_ascii.clone();

        std::thread::spawn(move || {
            let repo = match Repository::open(&repo_path) {
                Ok(r) => r,
                Err(e) => {
                    let _ = tx.send(Err(e.to_string()));
                    return;
                }
            };

            let mut out: Vec<u32> = Vec::new();
            for i in start..end {
                if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                    let _ = tx.send(Err("Cancelled".to_string()));
                    return;
                }
                let oid = oids[i];
                let commit = match repo.find_commit(oid) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.send(Err(e.to_string()));
                        return;
                    }
                };

                let msg_bytes = commit.message_bytes();
                let matched = if let Some(ref needle_lower) = needle_lower_ascii {
                    contains_ascii_case_insensitive(msg_bytes, needle_lower)
                } else {
                    let msg = String::from_utf8_lossy(msg_bytes);
                    msg.to_lowercase().contains(&query_lower)
                };

                if matched {
                    out.push(i as u32);
                }
            }
            let _ = tx.send(Ok(out));
        });
    }
    drop(tx);

    let mut all: Vec<u32> = Vec::new();
    for res in rx {
        match res {
            Ok(mut v) => all.append(&mut v),
            Err(e) => return Err(e),
        }
    }
    all.sort_unstable();
    Ok(all)
}
