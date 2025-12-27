use crate::git;
use crate::logger::Logger;
use gtk::{gio, glib, prelude::*};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

// Search state: (search_query, matching_indices, current_index)
pub type SearchState = Arc<Mutex<(String, Vec<u32>, usize)>>;

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
}

impl SearchHandler {
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new((String::new(), Vec::new(), 0))),
        }
    }

    /// Find matching commit indices (revwalk order) from git repository.
    pub fn find_matching_indices_in_repo(
        path: &PathBuf,
        branch_ref: &str,
        query: &str,
    ) -> Result<Vec<u32>, String> {
        Self::find_matching_indices_in_repo_cancelable(path, branch_ref, query, None)
    }

    pub fn find_matching_indices_in_repo_cancelable(
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

        let repo_result = if let Some(cancel) = cancel.clone() {
            git::read_git_tree_from_path_cancelable(path.to_str().unwrap(), branch_ref, cancel)
        } else {
            git::read_git_tree_from_path(path.to_str().unwrap(), branch_ref)
        };

        match repo_result {
            Ok(result) => {
                let mut matching_indices: Vec<u32> = Vec::new();
                for (i, commit) in result.commits.iter().enumerate() {
                    if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                        return Err("Cancelled".to_string());
                    }
                    let summary_lower = commit.summary.to_lowercase();
                    let sha_lower = commit.id.to_lowercase();
                    if summary_lower.contains(&query_lower) || sha_lower.contains(&query_lower) {
                        // Safety: commit lists larger than u32::MAX are not realistic here.
                        matching_indices.push(i as u32);
                    }
                }
                Ok(matching_indices)
            }
            Err(e) => {
                let msg = e.message().to_string();
                if msg.contains("Cancelled") {
                    Err("Cancelled".to_string())
                } else {
                    Err(format!("Error reading git commits for search: {}", e))
                }
            }
        }
    }

    pub fn perform_search_async_cancelable(
        path: PathBuf,
        branch_name: Option<String>,
        query: String,
        cancel: Option<Arc<AtomicBool>>,
    ) -> std::sync::mpsc::Receiver<SearchResult> {
        let (tx, rx) = std::sync::mpsc::channel();

        std::thread::spawn(move || {
            let query_text = query.clone();
            let start_time = std::time::Instant::now();
            Logger::info(&format!("Search query started: \"{}\"", query_text));

            if cancel.as_ref().is_some_and(|c| c.load(Ordering::Relaxed)) {
                Logger::info(&format!("Search query cancelled: \"{}\"", query_text));
                return;
            }

            let result = match Self::find_matching_indices_in_repo_cancelable(
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
            let indices = Self::find_matching_indices_in_repo(path, branch_ref, &query).ok()?;
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
            let indices = Self::find_matching_indices_in_repo(path, branch_ref, &query).ok()?;
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
