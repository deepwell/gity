//! Commit list UI component for displaying git commits with infinite scroll.
//!
//! This module provides the `CommitList` widget which displays a paginated,
//! scrollable list of git commits with columns for message, author, SHA, and date.

use gtk::prelude::*;
use gtk::{gio, glib};
use std::cell::Ref;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, mpsc};
use std::time::Instant;

use crate::git::{self, GitCommit};
use crate::logger::Logger;
use crate::ui::{Entry, GridCell};

/// Number of commits to load per page during infinite scroll.
const COMMIT_PAGE_SIZE: usize = 200;

// =============================================================================
// Public types
// =============================================================================

/// Messages sent from UI to the background commit loading worker.
#[derive(Debug)]
pub enum CommitLoadRequest {
    /// Request the next page of commits.
    NextPage,
    /// Stop the worker thread.
    Stop,
}

/// State tracking for paginated commit loading.
#[derive(Default)]
pub struct CommitPagingState {
    /// Current generation ID (incremented on each new load to invalidate stale results).
    pub generation: u64,
    /// Whether a page load is currently in progress.
    pub is_loading: bool,
    /// Whether all commits have been loaded.
    pub done: bool,
    /// Channel to send requests to the worker thread.
    pub request_tx: Option<mpsc::Sender<CommitLoadRequest>>,
    /// Source ID for the polling timeout (for cleanup).
    pub result_source: Option<glib::SourceId>,
    /// Optional performance logging for first page load.
    pub pending_first_page_log: Option<(Instant, PathBuf, String)>,
}

/// A scrollable list widget displaying git commits with infinite scroll support.
///
/// The widget displays commits in a column view with:
/// - Commit message
/// - Author name
/// - SHA (abbreviated)
/// - Date
///
/// Commits are loaded on-demand as the user scrolls, with pages loaded
/// in a background thread to keep the UI responsive.
#[derive(Clone)]
pub struct CommitList {
    /// The root scrolled window widget.
    pub widget: gtk::ScrolledWindow,
    /// Selection model for single-commit selection.
    pub selection_model: gtk::SingleSelection,
    /// The underlying data store.
    pub store: gio::ListStore,
    /// Shared paging state.
    paging_state: Rc<std::cell::RefCell<CommitPagingState>>,
    /// Generation counter for invalidating stale loads.
    generation_counter: Arc<AtomicU64>,
}

impl CommitList {
    /// Create a new CommitList widget.
    pub fn new() -> Self {
        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let selection_model = gtk::SingleSelection::new(Some(store.clone()));
        let column_view = gtk::ColumnView::new(Some(selection_model.clone()));

        // Create column factories
        let message_column = create_column("Message", 600, true, |c: &GitCommit| c.message.clone());
        let author_column = create_column("Author", 150, false, |c: &GitCommit| c.author.clone());
        let sha_column = create_column("SHA", 120, false, |c: &GitCommit| c.id.clone());
        let date_column = create_column("Date", 200, false, |c: &GitCommit| c.date.clone());

        column_view.append_column(&message_column);
        column_view.append_column(&author_column);
        column_view.append_column(&sha_column);
        column_view.append_column(&date_column);

        let scrolled_window = gtk::ScrolledWindow::builder().build();
        scrolled_window.set_child(Some(&column_view));
        scrolled_window.set_vexpand(true);
        scrolled_window.set_hexpand(true);

        let paging_state = Rc::new(std::cell::RefCell::new(CommitPagingState::default()));
        let generation_counter = Arc::new(AtomicU64::new(0));

        // Infinite scroll: request the next page when nearing the bottom.
        setup_infinite_scroll(&scrolled_window, &paging_state);

        Self {
            widget: scrolled_window,
            selection_model,
            store,
            paging_state,
            generation_counter,
        }
    }

    /// Load commits from a repository branch.
    ///
    /// # Arguments
    /// * `path` - Path to the git repository
    /// * `branch_ref` - Branch reference to load commits from
    /// * `on_first_page_branch` - Callback invoked with the branch name when the first page loads
    pub fn load_commits(
        &self,
        path: PathBuf,
        branch_ref: String,
        on_first_page_branch: impl Fn(String) + 'static,
    ) {
        let on_first_page_branch: Rc<dyn Fn(String)> = Rc::new(on_first_page_branch);
        start_commit_paging(
            &self.store,
            &self.selection_model,
            &self.paging_state,
            &self.generation_counter,
            path,
            branch_ref,
            on_first_page_branch,
        );
    }

    /// Get a clone of the paging state for external access.
    pub fn paging_state(&self) -> Rc<std::cell::RefCell<CommitPagingState>> {
        self.paging_state.clone()
    }

    /// Clear all commits and stop any in-flight loading.
    pub fn clear(&self) {
        // Invalidate any in-flight paging generation and stop the worker.
        let generation = self.generation_counter.fetch_add(1, Ordering::SeqCst) + 1;
        {
            let mut st = self.paging_state.borrow_mut();
            st.generation = generation;
            if let Some(tx) = st.request_tx.take() {
                let _ = tx.send(CommitLoadRequest::Stop);
            }
            if let Some(source) = st.result_source.take() {
                source.remove();
            }
            st.is_loading = false;
            st.done = true;
            st.pending_first_page_log = None;
        }

        // Clear UI list + selection.
        self.store.remove_all();
        self.selection_model
            .set_selected(gtk::INVALID_LIST_POSITION);
    }
}

impl Default for CommitList {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Private types
// =============================================================================

/// Messages sent from the background worker to the UI.
#[derive(Debug)]
enum CommitLoadResponse {
    /// A page of commits was loaded successfully.
    Page {
        generation: u64,
        branch_name: String,
        commits: Vec<GitCommit>,
        done: bool,
        is_first_page: bool,
    },
    /// An error occurred while loading commits.
    Error { generation: u64, message: String },
}

// =============================================================================
// Private helper functions
// =============================================================================

/// Create a column for the commit list view.
fn create_column<F>(title: &str, width: i32, expand: bool, extractor: F) -> gtk::ColumnViewColumn
where
    F: Fn(&GitCommit) -> String + 'static + Clone,
{
    let factory = gtk::SignalListItemFactory::new();

    factory.connect_setup(|_factory, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = GridCell::default();
        item.set_child(Some(&row));
    });

    let extractor_for_bind = extractor.clone();
    factory.connect_bind(move |_factory, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let child = item.child().and_downcast::<GridCell>().unwrap();
        let entry_obj = item.item().and_downcast::<glib::BoxedAnyObject>().unwrap();
        let commit: Ref<GitCommit> = entry_obj.borrow();
        let ent = Entry {
            name: extractor_for_bind(&commit),
        };
        child.set_entry(&ent);
    });

    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_resizable(true);
    column.set_fixed_width(width);
    if expand {
        column.set_expand(true);
    }
    column
}

/// Set up infinite scroll behavior on the scrolled window.
fn setup_infinite_scroll(
    scrolled_window: &gtk::ScrolledWindow,
    paging_state: &Rc<std::cell::RefCell<CommitPagingState>>,
) {
    let paging_state_for_scroll = paging_state.clone();
    scrolled_window
        .vadjustment()
        .connect_value_changed(move |adj| {
            let value = adj.value();
            let upper = adj.upper();
            let page_size = adj.page_size();

            // Nothing scrollable yet (or still laying out).
            if upper <= 0.0 || page_size <= 0.0 {
                return;
            }

            // Trigger when within ~half a page of the bottom.
            let near_bottom = value + page_size >= upper - (page_size * 0.5);
            if !near_bottom {
                return;
            }

            let mut st = paging_state_for_scroll.borrow_mut();
            if st.done || st.is_loading {
                return;
            }
            let Some(tx) = st.request_tx.clone() else {
                return;
            };
            st.is_loading = true;
            let _ = tx.send(CommitLoadRequest::NextPage);
        });
}

/// Poll for commit page results from the background worker.
fn poll_commit_pages(
    rx: mpsc::Receiver<CommitLoadResponse>,
    expected_generation: u64,
    store: gio::ListStore,
    selection_model: gtk::SingleSelection,
    paging_state: Rc<std::cell::RefCell<CommitPagingState>>,
    repo_path: PathBuf,
    on_first_page_branch: Rc<dyn Fn(String)>,
) {
    // Stop polling if a newer generation started.
    if paging_state.borrow().generation != expected_generation {
        return;
    }

    loop {
        match rx.try_recv() {
            Ok(CommitLoadResponse::Page {
                generation,
                branch_name,
                commits,
                done,
                is_first_page,
            }) => {
                if generation != expected_generation {
                    continue;
                }

                for commit in commits {
                    store.append(&glib::BoxedAnyObject::new(GitCommit {
                        id: commit.id,
                        message: commit.message,
                        author: commit.author,
                        date: commit.date,
                    }));
                }

                {
                    let mut st = paging_state.borrow_mut();
                    st.is_loading = false;
                    st.done = done;
                }

                if is_first_page {
                    (on_first_page_branch)(branch_name);

                    // Auto-select first commit (triggers diff load).
                    if store.n_items() > 0 {
                        selection_model.select_item(0, true);
                    }

                    // Optional "open repo" perf log (first page only).
                    let pending_log = paging_state.borrow_mut().pending_first_page_log.take();
                    if let Some((started_at, repo_path, label)) = pending_log {
                        glib::idle_add_local_once(move || {
                            Logger::info(&format!(
                                "{}: {}ms ({})",
                                label,
                                started_at.elapsed().as_millis(),
                                repo_path.display()
                            ));
                        });
                    }
                }
            }
            Ok(CommitLoadResponse::Error {
                generation,
                message,
            }) => {
                if generation != expected_generation {
                    continue;
                }
                paging_state.borrow_mut().is_loading = false;
                Logger::error(&format!("Error loading commits: {}", message));
            }
            Err(mpsc::TryRecvError::Empty) => break,
            Err(mpsc::TryRecvError::Disconnected) => {
                paging_state.borrow_mut().is_loading = false;
                break;
            }
        }
    }

    if paging_state.borrow().generation != expected_generation {
        return;
    }

    // Schedule the next poll.
    let store_clone = store.clone();
    let selection_model_clone = selection_model.clone();
    let paging_state_clone = paging_state.clone();
    let repo_path_clone = repo_path.clone();
    let on_first_page_branch_clone = on_first_page_branch.clone();
    let source_id = glib::timeout_add_local_once(std::time::Duration::from_millis(30), move || {
        poll_commit_pages(
            rx,
            expected_generation,
            store_clone,
            selection_model_clone,
            paging_state_clone,
            repo_path_clone,
            on_first_page_branch_clone,
        );
    });
    paging_state.borrow_mut().result_source = Some(source_id);
}

/// Start the commit paging process for a repository.
fn start_commit_paging(
    store: &gio::ListStore,
    selection_model: &gtk::SingleSelection,
    paging_state: &Rc<std::cell::RefCell<CommitPagingState>>,
    generation_counter: &Arc<AtomicU64>,
    path: PathBuf,
    branch_ref: String,
    on_first_page_branch: Rc<dyn Fn(String)>,
) {
    // Clear existing items + cancel any in-flight worker.
    store.remove_all();
    selection_model.set_selected(gtk::INVALID_LIST_POSITION);
    {
        let mut st = paging_state.borrow_mut();
        if let Some(tx) = st.request_tx.take() {
            let _ = tx.send(CommitLoadRequest::Stop);
        }
        if let Some(source) = st.result_source.take() {
            source.remove();
        }
        st.is_loading = false;
        st.done = false;
    }

    // New generation for this load.
    let generation = generation_counter.fetch_add(1, Ordering::SeqCst) + 1;
    paging_state.borrow_mut().generation = generation;

    let (req_tx, req_rx) = mpsc::channel::<CommitLoadRequest>();
    let (res_tx, res_rx) = mpsc::channel::<CommitLoadResponse>();
    let expected_generation = generation;

    {
        let mut st = paging_state.borrow_mut();
        st.request_tx = Some(req_tx.clone());
        st.result_source = None;
    }

    // Start polling for commit pages on the main thread.
    poll_commit_pages(
        res_rx,
        expected_generation,
        store.clone(),
        selection_model.clone(),
        paging_state.clone(),
        path.clone(),
        on_first_page_branch,
    );

    // Worker thread: create revwalk once, then emit pages when requested (scroll-driven).
    let path_for_thread = path.clone();
    let branch_for_thread = branch_ref.clone();
    let generation_counter_for_thread = generation_counter.clone();
    std::thread::spawn(move || {
        let commit_ref = branch_for_thread;
        let actual_branch_name = commit_ref.clone();

        let repo = match git2::Repository::open(&path_for_thread) {
            Ok(r) => r,
            Err(e) => {
                let _ = res_tx.send(CommitLoadResponse::Error {
                    generation: expected_generation,
                    message: e.to_string(),
                });
                return;
            }
        };

        let opts = git::CommitQueryOptions::for_branch(&commit_ref);
        let mut walker = match git::CommitWalker::new(&repo, opts) {
            Ok(w) => w,
            Err(e) => {
                let _ = res_tx.send(CommitLoadResponse::Error {
                    generation: expected_generation,
                    message: e.to_string(),
                });
                return;
            }
        };

        let mut is_first_page = true;
        loop {
            match req_rx.recv() {
                Ok(CommitLoadRequest::NextPage) => {
                    if generation_counter_for_thread.load(Ordering::Relaxed) != expected_generation
                    {
                        break;
                    }

                    let (commits, done) = match walker.next_page(COMMIT_PAGE_SIZE, None) {
                        Ok(v) => v,
                        Err(e) => {
                            let _ = res_tx.send(CommitLoadResponse::Error {
                                generation: expected_generation,
                                message: e.to_string(),
                            });
                            return;
                        }
                    };

                    let _ = res_tx.send(CommitLoadResponse::Page {
                        generation: expected_generation,
                        branch_name: actual_branch_name.clone(),
                        commits,
                        done,
                        is_first_page,
                    });

                    is_first_page = false;
                    if done {
                        break;
                    }
                }
                Ok(CommitLoadRequest::Stop) | Err(_) => break,
            }
        }
    });

    // Kick off first page immediately.
    paging_state.borrow_mut().is_loading = true;
    let _ = req_tx.send(CommitLoadRequest::NextPage);
}
