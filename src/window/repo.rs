use gtk::{gio, glib, prelude::*};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::git;
use crate::logger::Logger;
use crate::ui::RefType;

use super::recent_repos;
use super::state::AppState;
use super::ui::WindowUi;

pub fn show_repo_error(window: &gtk::ApplicationWindow, path: &PathBuf, error: &str) {
    let dialog = gtk::AlertDialog::builder()
        .message("Not a Git repository")
        .detail(&format!(
            "“{}” is not a Git repository.\n\n{}",
            path.display(),
            error
        ))
        .build();
    dialog.show(Some(window));
}

pub fn load_repo(
    ui: &WindowUi,
    state: &AppState,
    app_name: &str,
    path: PathBuf,
    ref_name: Option<String>,
) {
    if let Err(e) = git::validate_repository(&path) {
        Logger::error(&format!("Failed to open repository: {}", e));
        close_repo(ui, state, app_name);
        return;
    }

    *state.current_path.borrow_mut() = Some(path.clone());

    let checked_out_branch = git::checked_out_branch_name(&path);
    let mut effective_ref = ref_name.unwrap_or_else(|| git::default_branch_ref(&path));

    // If the requested branch doesn't exist (e.g., was deleted), fall back to "main"
    if !git::branch_exists(&path, &effective_ref) {
        let default_branch = git::default_branch_ref(&path);
        if git::branch_exists(&path, &default_branch) {
            effective_ref = default_branch;
            Logger::info(&format!(
                "Branch not found, falling back to default branch: {}",
                effective_ref
            ));
        } else {
            effective_ref = "HEAD".to_string();
            Logger::error("No valid branch found, using HEAD");
        }
    }

    // Pre-set current ref so programmatic selection doesn't trigger redundant reloads.
    *state.current_ref.borrow_mut() = Some(effective_ref.clone());
    *state.current_ref_type.borrow_mut() = Some(RefType::Branch);

    // Update window title with folder name
    let folder_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_else(|| path.to_str().unwrap_or("Unknown"));
    ui.title_label
        .set_text(&format!("{} - {}", app_name, folder_name));

    // Load tags for commit list display (SHA -> tag names mapping)
    match git::get_tags(&path) {
        Ok(tags) => {
            ui.repo_view.commit_list.set_tags(tags);
        }
        Err(e) => Logger::error(&format!("Error loading tags: {}", e)),
    }

    // Show upstream chip on the commit where the upstream branch points (if any)
    ui.repo_view
        .commit_list
        .set_upstream(git::get_branch_upstream(&path, &effective_ref));

    ui.repo_view
        .commit_list
        .load_commits(path.clone(), effective_ref.clone(), {
            let current_ref = state.current_ref.clone();
            move |ref_name| {
                *current_ref.borrow_mut() = Some(ref_name);
            }
        });

    // Load branches and tags for the branch panel
    let branches = match git::get_local_branches(path.to_str().unwrap()) {
        Ok(b) => b,
        Err(e) => {
            Logger::error(&format!("Error reading branches: {}", e));
            Vec::new()
        }
    };

    let tags = match git::get_tag_list(&path) {
        Ok(t) => t,
        Err(e) => {
            Logger::error(&format!("Error reading tags: {}", e));
            Vec::new()
        }
    };

    ui.repo_view.branch_panel.update_refs(
        &branches,
        &tags,
        checked_out_branch.as_deref(),
        Some(&effective_ref),
    );
    let _ = ui.repo_view.branch_panel.select_ref(&effective_ref);
}

/// Switch to a different branch or tag within the same repository.
///
/// This is more efficient than `load_repo` as it only reloads the commit list,
/// reusing already-loaded tags (which are global to the repository).
pub fn switch_ref(ui: &WindowUi, state: &AppState, ref_name: &str, ref_type: RefType) {
    let Some(path) = state.current_path.borrow().clone() else {
        Logger::error("Cannot switch ref: no repository loaded");
        return;
    };

    // Update current ref state
    *state.current_ref.borrow_mut() = Some(ref_name.to_string());
    *state.current_ref_type.borrow_mut() = Some(ref_type);

    // Reload only the commit list (tags are already loaded and don't change per-ref)
    ui.repo_view
        .commit_list
        .set_upstream(git::get_branch_upstream(&path, ref_name));
    ui.repo_view
        .commit_list
        .load_commits(path, ref_name.to_string(), {
            let current_ref = state.current_ref.clone();
            move |ref_name| {
                *current_ref.borrow_mut() = Some(ref_name);
            }
        });
}

pub fn open_repo_dialog(
    window: &gtk::ApplicationWindow,
    ui: &WindowUi,
    state: &AppState,
    app_name: &str,
) {
    // Check if a file portal dialog is already active
    let is_active = *state.file_portal_active.borrow();

    if is_active {
        Logger::debug("Dialog already active, returning early");
        return;
    }

    // Mark dialog as active
    *state.file_portal_active.borrow_mut() = true;

    let window_for_dialog = window.clone();
    let ui_for_dialog = ui.clone();
    let state_for_dialog = state.clone();
    let app_name = app_name.to_string();

    // Use ashpd file portal to open a folder
    // Use the shared Tokio runtime from AppState to ensure proper DBus connection management
    // Use a channel to send results back to the main thread
    let (tx, rx) = std::sync::mpsc::channel();

    // Clone the shared runtime for use in the background thread
    let shared_runtime = state.tokio_runtime.clone();

    std::thread::spawn(move || {
        shared_runtime.block_on(async {
            use ashpd::desktop::file_chooser::SelectedFiles;
            use tokio::time::{Duration as TokioDuration, timeout};

            let request_builder = SelectedFiles::open_file()
                .directory(true)
                .title("Select Repository")
                .modal(true);

            // Add a 60 second timeout to detect if the dialog never appears
            let result = match timeout(TokioDuration::from_secs(60), request_builder.send()).await {
                Ok(result) => result,
                Err(_) => {
                    let _ = tx.send(Err(
                        "File portal dialog timed out - it may not have appeared".to_string(),
                    ));
                    return;
                }
            };

            match result {
                Ok(request) => {
                    match request.response() {
                        Ok(files) => {
                            // Get the first selected file/folder
                            let uris = files.uris();
                            if let Some(uri) = uris.first() {
                                Logger::debug(&format!("Got URI: {}", uri));
                                // Convert URI to path - Url implements Display/Debug, convert to string
                                let uri_str = uri.to_string();
                                let file = gio::File::for_uri(&uri_str);
                                if let Some(sandbox_path) = file.path() {
                                    // Try to get the real path from the document portal extended attribute
                                    // This works in Flatpak/sandboxed environments
                                    let real_path = if let Ok(info) = file.query_info(
                                        "xattr::document-portal.host-path",
                                        gio::FileQueryInfoFlags::NONE,
                                        gio::Cancellable::NONE,
                                    ) {
                                        if let Some(host_path) = info
                                            .attribute_as_string("xattr::document-portal.host-path")
                                        {
                                            PathBuf::from(host_path)
                                        } else {
                                            sandbox_path.clone()
                                        }
                                    } else {
                                        // Fallback: use sandbox path as real path
                                        // This works for non-sandboxed environments where paths are the same
                                        sandbox_path.clone()
                                    };

                                    let _ = tx.send(Ok((sandbox_path, real_path)));
                                } else {
                                    let _ = tx.send(Err("Failed to get path from URI".to_string()));
                                }
                            } else {
                                // User cancelled or no file selected - send cancellation message
                                let _ = tx.send(Err("Dialog cancelled".to_string()));
                            }
                        }
                        Err(e) => {
                            // User cancelled or error occurred - always send a message
                            let error_msg = format!("File portal error: {}", e);
                            let _ = tx.send(Err(error_msg));
                        }
                    }
                }
                Err(e) => {
                    // Failed to open portal - always send a message
                    let _ = tx.send(Err(format!("Failed to open file portal: {}", e)));
                }
            }
        });
    });

    // Poll for the result on the main thread using glib timeout
    // Wrap receiver in Arc<Mutex> so it can be shared across closures
    let rx_shared = Arc::new(Mutex::new(rx));
    let window_for_poll = window_for_dialog.clone();
    let ui_for_poll = ui_for_dialog.clone();
    let state_for_poll = state_for_dialog.clone();
    let app_name_for_poll = app_name.clone();

    // Track when we started polling for timeout detection
    let poll_start_time = Instant::now();
    let poll_start_time_shared = Arc::new(Mutex::new(poll_start_time));

    poll_file_portal_result(
        rx_shared,
        poll_start_time_shared,
        &window_for_poll,
        &ui_for_poll,
        &state_for_poll,
        &app_name_for_poll,
    );
}

fn poll_file_portal_result(
    rx: Arc<Mutex<std::sync::mpsc::Receiver<Result<(PathBuf, PathBuf), String>>>>,
    poll_start_time: Arc<Mutex<Instant>>,
    window: &gtk::ApplicationWindow,
    ui: &WindowUi,
    state: &AppState,
    app_name: &str,
) {
    // Check for timeout (90 seconds max - longer than the 60s timeout in background thread)
    let elapsed = {
        let start_guard = poll_start_time.lock().unwrap();
        start_guard.elapsed()
    };

    if elapsed > Duration::from_secs(90) {
        Logger::debug("Polling timeout reached (90s), clearing flag and stopping");
        *state.file_portal_active.borrow_mut() = false;
        return;
    }

    let result = {
        let rx_guard = rx.lock().unwrap();
        rx_guard.try_recv()
    };

    match result {
        Ok(Ok((sandbox_path, real_path))) => {
            Logger::debug(&format!(
                "Received success: sandbox={:?}, real={:?}",
                sandbox_path, real_path
            ));
            // Clear the active flag
            *state.file_portal_active.borrow_mut() = false;

            // Validate repository
            if let Err(e) = git::validate_repository(&sandbox_path) {
                show_repo_error(window, &sandbox_path, &e.to_string());
                return;
            }

            // If a repository is already open, reset any repo-specific UI/state first
            if state.current_path.borrow().is_some() {
                reset_for_repo_switch(ui, state);
            }

            let started_at = Instant::now();
            ui.repo_view
                .commit_paging_state
                .borrow_mut()
                .pending_first_page_log = Some((
                started_at,
                sandbox_path.clone(),
                "Open repo load -> rendered on screen".to_string(),
            ));

            recent_repos::add_recent_repo(&sandbox_path, &real_path);
            load_repo(ui, state, app_name, sandbox_path, None);
            ui.set_repo_controls_visible(true);
            ui.show_main();
        }
        Ok(Err(e)) => {
            // Clear the active flag on error (including cancellation)
            *state.file_portal_active.borrow_mut() = false;
            Logger::error(&format!("File portal error: {}", e));
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            // Result not ready yet, poll again
            let window_for_retry = window.clone();
            let ui_for_retry = ui.clone();
            let state_for_retry = state.clone();
            let app_name_for_retry = app_name.to_string();
            let rx_for_retry = rx.clone();
            let poll_start_time_for_retry = poll_start_time.clone();

            glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
                poll_file_portal_result(
                    rx_for_retry,
                    poll_start_time_for_retry,
                    &window_for_retry,
                    &ui_for_retry,
                    &state_for_retry,
                    &app_name_for_retry,
                );
            });
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
            // Channel disconnected - clear the flag and stop polling
            *state.file_portal_active.borrow_mut() = false;
            Logger::error("File portal channel disconnected");
        }
    }
}

pub fn refresh_repo(ui: &WindowUi, state: &AppState, app_name: &str) {
    let path_opt = state.current_path.borrow().clone();
    let ref_opt = state.current_ref.borrow().clone();
    if let Some(path) = path_opt {
        // Clear diff UI before refreshing to avoid showing stale data
        // from the old commit list (especially important after commit amend)
        while let Some(child) = ui.repo_view.diff_files_box.first_child() {
            ui.repo_view.diff_files_box.remove(&child);
        }
        ui.repo_view.diff_label.set_text("Commit Diff");
        ui.repo_view.commit_message_label.set_text("");
        ui.repo_view.expand_label.set_visible(false);
        *ui.repo_view.full_message.borrow_mut() = String::new();
        *ui.repo_view.is_expanded.borrow_mut() = false;

        let started_at = Instant::now();
        ui.repo_view
            .commit_paging_state
            .borrow_mut()
            .pending_first_page_log = Some((
            started_at,
            path.clone(),
            "Refresh repo load -> rendered on screen".to_string(),
        ));
        load_repo(ui, state, app_name, path, ref_opt);
    }
}

pub fn close_repo(ui: &WindowUi, state: &AppState, app_name: &str) {
    state.clear_repo();
    ui.title_label.set_text(app_name);

    ui.repo_view.commit_list.clear();
    ui.repo_view.branch_panel.update_branches(&[], None);

    // Reset search UI
    ui.repo_view.search_bar.set_search_mode(false);
    ui.repo_view.search_entry.set_text("");
    ui.repo_view.search_status_label.set_text("");
    ui.repo_view.last_search_status.borrow_mut().clear();

    // Reset diff UI
    while let Some(child) = ui.repo_view.diff_files_box.first_child() {
        ui.repo_view.diff_files_box.remove(&child);
    }
    ui.repo_view.diff_files_box.append(
        &gtk::Label::builder()
            .label("No repository loaded")
            .halign(gtk::Align::Start)
            .wrap(true)
            .build(),
    );
    ui.repo_view.diff_label.set_text("Commit Diff");
    ui.repo_view.commit_message_label.set_text("");
    ui.repo_view.expand_label.set_visible(false);
    *ui.repo_view.full_message.borrow_mut() = String::new();
    *ui.repo_view.is_expanded.borrow_mut() = false;

    // Switch back to welcome screen and hide repo-only controls
    ui.set_repo_controls_visible(false);
    ui.refresh_recent_repos();
    ui.show_welcome();
}

pub fn reset_for_repo_switch(ui: &WindowUi, state: &AppState) {
    // Clear the old repo state; `load_repo` will set the new path.
    state.clear_repo();

    // Clear panels while the new repo loads.
    ui.repo_view.commit_list.clear();
    ui.repo_view.branch_panel.update_branches(&[], None);

    // Reset search UI.
    ui.repo_view.search_bar.set_search_mode(false);
    ui.repo_view.search_entry.set_text("");
    ui.repo_view.search_status_label.set_text("");
    ui.repo_view.last_search_status.borrow_mut().clear();

    // Reset diff UI.
    while let Some(child) = ui.repo_view.diff_files_box.first_child() {
        ui.repo_view.diff_files_box.remove(&child);
    }
    ui.repo_view.diff_label.set_text("Commit Diff");
    ui.repo_view.commit_message_label.set_text("");
    ui.repo_view.expand_label.set_visible(false);
    *ui.repo_view.full_message.borrow_mut() = String::new();
    *ui.repo_view.is_expanded.borrow_mut() = false;
}

pub fn maybe_load_repo_from_cwd(
    window: &gtk::ApplicationWindow,
    ui: &WindowUi,
    state: &AppState,
    app_name: &str,
) -> bool {
    // If launched from a terminal, auto-open the current working directory if it
    // is inside a Git repository (worktree root preferred).
    if let Ok(cwd) = std::env::current_dir() {
        if let Some(repo_root) = git::discover_repository_root(&cwd) {
            if repo_root.exists() {
                if let Err(e) = git::validate_repository(&repo_root) {
                    show_repo_error(window, &repo_root, &e.to_string());
                    ui.set_repo_controls_visible(false);
                    ui.show_welcome();
                    return false;
                }

                let started_at = Instant::now();
                ui.repo_view
                    .commit_paging_state
                    .borrow_mut()
                    .pending_first_page_log = Some((
                    started_at,
                    repo_root.clone(),
                    "Auto-open repo from CWD -> rendered on screen".to_string(),
                ));

                let branch = git::checked_out_branch_name(&repo_root);
                // For non-portal access, real_path and sandbox_path are the same
                recent_repos::add_recent_repo(&repo_root, &repo_root);
                ui.set_repo_controls_visible(true);
                ui.show_main();
                load_repo(ui, state, app_name, repo_root, branch);
                return true;
            }
        }
    }

    ui.set_repo_controls_visible(false);
    ui.show_welcome();
    false
}
