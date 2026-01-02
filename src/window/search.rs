use gtk::{gio, glib, prelude::*};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use crate::search::SearchHandler;
use crate::ui::{CommitLoadRequest, CommitPagingState};

use super::state::AppState;
use super::ui::WindowUi;

const MAX_SEARCH_QUERY_CHARS: usize = 100;

fn clamp_search_query(s: &str) -> String {
    s.chars().take(MAX_SEARCH_QUERY_CHARS).collect()
}

fn ensure_loaded_then_select(
    target_index: u32,
    expected_generation: u64,
    store: gio::ListStore,
    selection_model: gtk::SingleSelection,
    scrolled_window: gtk::ScrolledWindow,
    commit_paging_state: std::rc::Rc<std::cell::RefCell<CommitPagingState>>,
    attempt: u32,
) {
    // Abort if repo load generation changed (repo switched / reloaded).
    if commit_paging_state.borrow().generation != expected_generation {
        return;
    }

    if store.n_items() > target_index {
        selection_model.select_item(target_index, true);
        crate::search::SearchHandler::scroll_to_item(
            &scrolled_window,
            &selection_model,
            target_index,
        );
        return;
    }

    let mut st = commit_paging_state.borrow_mut();
    let can_page = !st.done && st.request_tx.is_some();
    if !can_page || attempt >= 200 {
        return;
    }
    if !st.is_loading {
        if let Some(tx) = st.request_tx.clone() {
            st.is_loading = true;
            let _ = tx.send(CommitLoadRequest::NextPage);
        }
    }
    drop(st);

    glib::timeout_add_local_once(std::time::Duration::from_millis(100), move || {
        ensure_loaded_then_select(
            target_index,
            expected_generation,
            store,
            selection_model,
            scrolled_window,
            commit_paging_state,
            attempt + 1,
        );
    });
}

#[derive(Clone)]
pub struct SearchController {
    handler: SearchHandler,
}

impl SearchController {
    pub fn connect(ui: &WindowUi, state: &AppState) -> Self {
        let handler = SearchHandler::new();
        let debounce_source: std::rc::Rc<std::cell::RefCell<Option<glib::SourceId>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));
        let current_cancel: std::rc::Rc<std::cell::RefCell<Option<Arc<AtomicBool>>>> =
            std::rc::Rc::new(std::cell::RefCell::new(None));

        // Cancel any in-flight search (and any pending debounce) when the search bar is hidden.
        // This covers both the hide action and the ESC key handler (which directly toggles
        // search mode on the bar).
        let debounce_source_for_hide = debounce_source.clone();
        let current_cancel_for_hide = current_cancel.clone();
        let search_spinner_for_hide = ui.repo_view.search_spinner.clone();
        ui.repo_view
            .search_bar
            .connect_search_mode_enabled_notify(move |bar| {
                if bar.is_search_mode() {
                    return;
                }

                // Cancel any pending debounce timeout.
                if let Some(source_id) = debounce_source_for_hide.borrow_mut().take() {
                    source_id.remove();
                }

                // Cancel any currently running background search.
                if let Some(token) = current_cancel_for_hide.borrow_mut().take() {
                    token.store(true, Ordering::Relaxed);
                }

                // Clear loading indicator immediately.
                search_spinner_for_hide.stop();
                search_spinner_for_hide.set_visible(false);
            });

        // Debounced search on text changes
        let store_for_changed = ui.repo_view.commit_list.store.clone();
        let selection_model_for_changed = ui.repo_view.commit_list.selection_model.clone();
        let scrolled_window_for_changed = ui.repo_view.commit_list.widget.clone();
        let handler_for_changed = handler.clone();
        let state_for_changed = state.clone();
        let debounce_source_for_changed = debounce_source.clone();
        let current_cancel_for_changed = current_cancel.clone();
        let search_status_label_for_changed = ui.repo_view.search_status_label.clone();
        let search_spinner_for_changed = ui.repo_view.search_spinner.clone();
        let last_search_status_for_changed = ui.repo_view.last_search_status.clone();
        let commit_paging_state_for_changed = ui.repo_view.commit_paging_state.clone();

        ui.repo_view.search_entry.connect_changed(move |entry| {
            let text = entry.text();

            // Cancel any pending debounce timeout
            if let Some(source_id) = debounce_source_for_changed.borrow_mut().take() {
                source_id.remove();
            }

            // If a search is currently running, request cancellation immediately.
            if let Some(token) = current_cancel_for_changed.borrow_mut().take() {
                token.store(true, Ordering::Relaxed);
            }
            // Clear the spinner immediately; the new search (if any) will re-enable it
            // when the debounce fires.
            search_spinner_for_changed.stop();
            search_spinner_for_changed.set_visible(false);

            if text.is_empty() {
                search_status_label_for_changed.set_text("");
                last_search_status_for_changed.borrow_mut().clear();
                return;
            }

            // Clamp the query used for searching, but do not modify the entry text.
            let query = clamp_search_query(text.as_str());
            if query.to_lowercase().is_empty() {
                search_status_label_for_changed.set_text("");
                last_search_status_for_changed.borrow_mut().clear();
                return;
            }

            // Get current path and branch
            let path_opt = state_for_changed.current_path.borrow().clone();
            let branch_opt = state_for_changed.current_branch.borrow().clone();
            if path_opt.is_none() {
                search_status_label_for_changed.set_text("");
                return;
            }
            let path = path_opt.unwrap();
            let branch_name = branch_opt.as_deref();

            // Clone all necessary values for the timeout closure
            let query_for_timeout = query.clone();
            let path_for_timeout = path.clone();
            let branch_name_for_timeout = branch_name.map(|s| s.to_string());
            let store_for_timeout = store_for_changed.clone();
            let selection_model_for_timeout = selection_model_for_changed.clone();
            let scrolled_window_for_timeout = scrolled_window_for_changed.clone();
            let handler_for_timeout = handler_for_changed.clone();
            let debounce_source_for_timeout = debounce_source_for_changed.clone();
            let current_cancel_for_timeout = current_cancel_for_changed.clone();
            let search_status_label_for_timeout = search_status_label_for_changed.clone();
            let search_spinner_for_timeout = search_spinner_for_changed.clone();
            let last_search_status_for_timeout = last_search_status_for_changed.clone();
            let commit_paging_state_for_timeout = commit_paging_state_for_changed.clone();
            let search_entry_for_timeout = entry.clone();

            // Debounce (200ms)
            let source_id =
                glib::timeout_add_local_once(std::time::Duration::from_millis(200), move || {
                    *debounce_source_for_timeout.borrow_mut() = None;

                    // Show loading indicator while the async search runs.
                    search_spinner_for_timeout.set_visible(true);
                    search_spinner_for_timeout.start();
                    search_status_label_for_timeout.set_text("");

                    // Start a new cancel token for this search.
                    let cancel_token = Arc::new(AtomicBool::new(false));
                    *current_cancel_for_timeout.borrow_mut() = Some(cancel_token.clone());

                    // Perform search in background thread
                    let rx = handler_for_timeout.perform_search_async_cancelable(
                        path_for_timeout.clone(),
                        branch_name_for_timeout.clone(),
                        query_for_timeout.clone(),
                        Some(cancel_token),
                    );

                    poll_search_result(
                        query_for_timeout.clone(),
                        rx,
                        handler_for_timeout.clone(),
                        store_for_timeout.clone(),
                        selection_model_for_timeout.clone(),
                        scrolled_window_for_timeout.clone(),
                        search_spinner_for_timeout.clone(),
                        search_status_label_for_timeout.clone(),
                        last_search_status_for_timeout.clone(),
                        commit_paging_state_for_timeout.clone(),
                        search_entry_for_timeout.clone(),
                    );
                });

            *debounce_source_for_changed.borrow_mut() = Some(source_id);
        });

        // Enter key finds next match
        let search_entry_for_enter = ui.repo_view.search_entry.clone();
        let store_for_enter = ui.repo_view.commit_list.store.clone();
        let selection_model_for_enter = ui.repo_view.commit_list.selection_model.clone();
        let scrolled_window_for_enter = ui.repo_view.commit_list.widget.clone();
        let commit_paging_state_for_enter = ui.repo_view.commit_paging_state.clone();
        let handler_for_enter = handler.clone();
        let state_for_enter = state.clone();
        let enter_key_controller = gtk::EventControllerKey::new();
        enter_key_controller.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Return || keyval == gtk::gdk::Key::KP_Enter {
                let text = search_entry_for_enter.text();
                if text.is_empty() {
                    return glib::Propagation::Stop;
                }
                let query = clamp_search_query(text.as_str());
                if query.to_lowercase().is_empty() {
                    return glib::Propagation::Stop;
                }

                let path_opt = state_for_enter.current_path.borrow().clone();
                let branch_opt = state_for_enter.current_branch.borrow().clone();
                if path_opt.is_none() {
                    return glib::Propagation::Stop;
                }
                let path = path_opt.unwrap();
                let branch_ref = branch_opt.as_deref().unwrap_or("HEAD");

                if let Some(target_index) =
                    handler_for_enter.compute_next_match_index(query, &path, branch_ref)
                {
                    let expected_generation = commit_paging_state_for_enter.borrow().generation;
                    ensure_loaded_then_select(
                        target_index,
                        expected_generation,
                        store_for_enter.clone(),
                        selection_model_for_enter.clone(),
                        scrolled_window_for_enter.clone(),
                        commit_paging_state_for_enter.clone(),
                        0,
                    );
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        ui.repo_view
            .search_entry
            .add_controller(enter_key_controller);

        Self { handler }
    }

    pub fn action_show_search(&self, ui: &WindowUi) -> gio::ActionEntry<gtk::ApplicationWindow> {
        let search_bar = ui.repo_view.search_bar.clone();
        let search_entry = ui.repo_view.search_entry.clone();
        let search_status_label = ui.repo_view.search_status_label.clone();
        let last_search_status = ui.repo_view.last_search_status.clone();
        gio::ActionEntry::builder("show-search")
            .activate(move |_, _, _| {
                search_bar.set_search_mode(true);
                search_entry.grab_focus();

                let text = search_entry.text();
                if !text.is_empty() {
                    let status = last_search_status.borrow().clone();
                    if !status.is_empty() {
                        search_status_label.set_text(&status);
                    }
                }
            })
            .build()
    }

    pub fn action_hide_search(&self, ui: &WindowUi) -> gio::ActionEntry<gtk::ApplicationWindow> {
        let search_bar = ui.repo_view.search_bar.clone();
        gio::ActionEntry::builder("hide-search")
            .activate(move |_, _, _| {
                if search_bar.is_search_mode() {
                    search_bar.set_search_mode(false);
                }
            })
            .build()
    }

    pub fn action_find_next(
        &self,
        ui: &WindowUi,
        state: &AppState,
    ) -> gio::ActionEntry<gtk::ApplicationWindow> {
        let search_entry = ui.repo_view.search_entry.clone();
        let store = ui.repo_view.commit_list.store.clone();
        let selection_model = ui.repo_view.commit_list.selection_model.clone();
        let scrolled_window = ui.repo_view.commit_list.widget.clone();
        let commit_paging_state = ui.repo_view.commit_paging_state.clone();
        let handler = self.handler.clone();
        let state = state.clone();
        gio::ActionEntry::builder("find-next")
            .activate(move |_, _, _| {
                let text = search_entry.text();
                if text.is_empty() {
                    return;
                }
                let query = clamp_search_query(text.as_str());
                if query.to_lowercase().is_empty() {
                    return;
                }

                let path_opt = state.current_path.borrow().clone();
                let branch_opt = state.current_branch.borrow().clone();
                if path_opt.is_none() {
                    return;
                }
                let path = path_opt.unwrap();
                let branch_ref = branch_opt.as_deref().unwrap_or("HEAD");

                if let Some(target_index) =
                    handler.compute_next_match_index(query, &path, branch_ref)
                {
                    let expected_generation = commit_paging_state.borrow().generation;
                    ensure_loaded_then_select(
                        target_index,
                        expected_generation,
                        store.clone(),
                        selection_model.clone(),
                        scrolled_window.clone(),
                        commit_paging_state.clone(),
                        0,
                    );
                }
            })
            .build()
    }

    pub fn action_find_previous(
        &self,
        ui: &WindowUi,
        state: &AppState,
    ) -> gio::ActionEntry<gtk::ApplicationWindow> {
        let search_entry = ui.repo_view.search_entry.clone();
        let store = ui.repo_view.commit_list.store.clone();
        let selection_model = ui.repo_view.commit_list.selection_model.clone();
        let scrolled_window = ui.repo_view.commit_list.widget.clone();
        let commit_paging_state = ui.repo_view.commit_paging_state.clone();
        let handler = self.handler.clone();
        let state = state.clone();
        gio::ActionEntry::builder("find-previous")
            .activate(move |_, _, _| {
                let text = search_entry.text();
                if text.is_empty() {
                    return;
                }
                let query = clamp_search_query(text.as_str());
                if query.to_lowercase().is_empty() {
                    return;
                }

                let path_opt = state.current_path.borrow().clone();
                let branch_opt = state.current_branch.borrow().clone();
                if path_opt.is_none() {
                    return;
                }
                let path = path_opt.unwrap();
                let branch_ref = branch_opt.as_deref().unwrap_or("HEAD");

                if let Some(target_index) =
                    handler.compute_previous_match_index(query, &path, branch_ref)
                {
                    let expected_generation = commit_paging_state.borrow().generation;
                    ensure_loaded_then_select(
                        target_index,
                        expected_generation,
                        store.clone(),
                        selection_model.clone(),
                        scrolled_window.clone(),
                        commit_paging_state.clone(),
                        0,
                    );
                }
            })
            .build()
    }
}

fn poll_search_result(
    expected_query: String,
    rx: mpsc::Receiver<crate::search::SearchResult>,
    search_handler: crate::search::SearchHandler,
    store: gio::ListStore,
    selection_model: gtk::SingleSelection,
    scrolled_window: gtk::ScrolledWindow,
    search_spinner: gtk::Spinner,
    search_status_label: gtk::Label,
    last_search_status: std::rc::Rc<std::cell::RefCell<String>>,
    commit_paging_state: std::rc::Rc<std::cell::RefCell<CommitPagingState>>,
    search_entry: gtk::Entry,
) {
    fn try_process_search_result(
        expected_query: String,
        result: crate::search::SearchResult,
        search_handler: crate::search::SearchHandler,
        store: gio::ListStore,
        selection_model: gtk::SingleSelection,
        scrolled_window: gtk::ScrolledWindow,
        search_spinner: gtk::Spinner,
        search_status_label: gtk::Label,
        last_search_status: std::rc::Rc<std::cell::RefCell<String>>,
        commit_paging_state: std::rc::Rc<std::cell::RefCell<CommitPagingState>>,
        search_entry: gtk::Entry,
        attempt: u32,
    ) {
        // Ignore stale results (e.g. query changed or repo switched) so we don't
        // overwrite the UI with an out-of-date search.
        // NOTE: we clamp the UI text before comparing, because the entry may contain
        // more than MAX_SEARCH_QUERY_CHARS but searches always run on the clamped query.
        if clamp_search_query(search_entry.text().as_str()) != expected_query {
            return;
        }

        // Ensure the first match is present in the paged store before processing the result.
        // Search matches are global indices (revwalk order), while the UI list is loaded lazily.
        if result.error.is_none() && !result.matching_indices.is_empty() {
            let first_index = result.matching_indices[0];
            if store.n_items() <= first_index {
                let mut st = commit_paging_state.borrow_mut();
                let can_page = !st.done && st.request_tx.is_some();
                if can_page && attempt < 200 {
                    if !st.is_loading {
                        if let Some(tx) = st.request_tx.clone() {
                            st.is_loading = true;
                            let _ = tx.send(CommitLoadRequest::NextPage);
                        }
                    }
                    drop(st);

                    let search_handler_clone = search_handler.clone();
                    let store_clone = store.clone();
                    let selection_model_clone = selection_model.clone();
                    let scrolled_window_clone = scrolled_window.clone();
                    let search_status_label_clone = search_status_label.clone();
                    let last_search_status_clone = last_search_status.clone();
                    let commit_paging_state_clone = commit_paging_state.clone();
                    let search_entry_clone = search_entry.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(100),
                        move || {
                            try_process_search_result(
                                expected_query,
                                result,
                                search_handler_clone,
                                store_clone,
                                selection_model_clone,
                                scrolled_window_clone,
                                search_spinner,
                                search_status_label_clone,
                                last_search_status_clone,
                                commit_paging_state_clone,
                                search_entry_clone,
                                attempt + 1,
                            );
                        },
                    );
                    return;
                }
            }
        }

        let search_status_label_clone = search_status_label.clone();
        let last_search_status_clone = last_search_status.clone();
        let search_spinner_clone = search_spinner.clone();
        search_handler.process_search_result_async(
            result,
            store,
            selection_model,
            scrolled_window,
            move |match_count| {
                let status_text = match match_count {
                    Some(count) => {
                        if count == 0 {
                            String::from("0 matches")
                        } else {
                            let count_text = format_usize_with_thousands(count);
                            format!("{} match{}", count_text, if count == 1 { "" } else { "es" })
                        }
                    }
                    None => String::new(),
                };
                search_status_label_clone.set_text(&status_text);
                *last_search_status_clone.borrow_mut() = status_text;
                search_spinner_clone.stop();
                search_spinner_clone.set_visible(false);
            },
        );
    }

    match rx.try_recv() {
        Ok(result) => {
            try_process_search_result(
                expected_query,
                result,
                search_handler,
                store,
                selection_model,
                scrolled_window,
                search_spinner,
                search_status_label,
                last_search_status,
                commit_paging_state,
                search_entry,
                0,
            );
        }
        Err(mpsc::TryRecvError::Empty) => {
            let search_handler_clone = search_handler.clone();
            let store_clone = store.clone();
            let selection_model_clone = selection_model.clone();
            let scrolled_window_clone = scrolled_window.clone();
            let search_spinner_clone = search_spinner.clone();
            let search_status_label_clone = search_status_label.clone();
            let last_search_status_clone = last_search_status.clone();
            let commit_paging_state_clone = commit_paging_state.clone();
            let search_entry_clone = search_entry.clone();
            let expected_query_clone = expected_query.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
                poll_search_result(
                    expected_query_clone,
                    rx,
                    search_handler_clone,
                    store_clone,
                    selection_model_clone,
                    scrolled_window_clone,
                    search_spinner_clone,
                    search_status_label_clone,
                    last_search_status_clone,
                    commit_paging_state_clone,
                    search_entry_clone,
                );
            });
        }
        Err(_) => {
            // Channel closed; clear loading indicator only if this is still the active query.
            if clamp_search_query(search_entry.text().as_str()) == expected_query {
                search_spinner.stop();
                search_spinner.set_visible(false);
                search_status_label.set_text("");
            }
        }
    }
}

fn format_usize_with_thousands(n: usize) -> String {
    // Simple, dependency-free thousands separator formatting (e.g. 12345 -> "12,345").
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut rev_out = String::with_capacity(s.len() + s.len() / 3);
    for (i, &b) in bytes.iter().rev().enumerate() {
        if i != 0 && i % 3 == 0 {
            rev_out.push(',');
        }
        rev_out.push(b as char);
    }
    rev_out.chars().rev().collect()
}
