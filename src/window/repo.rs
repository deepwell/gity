use gtk::{gio, prelude::*};
use std::path::PathBuf;
use std::time::Instant;

use crate::git;
use crate::logger::Logger;

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
    branch_name: Option<String>,
) {
    *state.current_path.borrow_mut() = Some(path.clone());

    let checked_out_branch = git::checked_out_branch_name(&path);
    let checked_out_tag = git::checked_out_tag_name(&path);
    let effective_branch = branch_name.unwrap_or_else(|| git::default_branch_ref(&path));

    // Pre-set current branch so programmatic selection doesn't trigger redundant reloads.
    *state.current_branch.borrow_mut() = Some(effective_branch.clone());

    // Update window title with folder name
    let folder_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_else(|| path.to_str().unwrap_or("Unknown"));
    ui.title_label
        .set_text(&format!("{} - {}", app_name, folder_name));

    ui.repo_view
        .commit_list
        .load_commits(path.clone(), effective_branch.clone(), {
            let current_branch = state.current_branch.clone();
            move |branch_name| {
                *current_branch.borrow_mut() = Some(branch_name);
            }
        });

    let path_str = path.to_str().unwrap();

    // Fetch branches
    let branches = match git::get_local_branches(path_str) {
        Ok(b) => b,
        Err(e) => {
            Logger::error(&format!("Error reading branches: {}", e));
            Vec::new()
        }
    };

    // Fetch tags
    let tags = match git::get_tags(path_str) {
        Ok(t) => t,
        Err(e) => {
            Logger::error(&format!("Error reading tags: {}", e));
            Vec::new()
        }
    };

    ui.repo_view
        .branch_panel
        .update_refs(&branches, &tags, checked_out_branch.as_deref(), checked_out_tag.as_deref());
    let _ = ui.repo_view.branch_panel.select_ref(&effective_branch);
}

pub fn open_repo_dialog(
    window: &gtk::ApplicationWindow,
    ui: &WindowUi,
    state: &AppState,
    app_name: &str,
) {
    let dialog = gtk::FileDialog::builder()
        .title("Select Repository")
        .build();

    let window_for_dialog = window.clone();
    let ui_for_dialog = ui.clone();
    let state_for_dialog = state.clone();
    let app_name = app_name.to_string();

    dialog.select_folder(Some(window), None::<&gio::Cancellable>, move |result| {
        if let Ok(file) = result {
            if let Some(path) = file.path() {
                if let Err(e) = git::validate_repository(&path) {
                    show_repo_error(&window_for_dialog, &path, &e.to_string());
                    return;
                }

                // If a repository is already open, reset any repo-specific UI/state first
                // so we don't carry over stale search/diff results.
                if state_for_dialog.current_path.borrow().is_some() {
                    reset_for_repo_switch(&ui_for_dialog, &state_for_dialog);
                }

                let started_at = Instant::now();
                ui_for_dialog
                    .repo_view
                    .commit_paging_state
                    .borrow_mut()
                    .pending_first_page_log = Some((
                    started_at,
                    path.clone(),
                    "Open repo load -> rendered on screen".to_string(),
                ));

                recent_repos::add_recent_repo(&path);
                load_repo(&ui_for_dialog, &state_for_dialog, &app_name, path, None);
                ui_for_dialog.set_repo_controls_visible(true);
                ui_for_dialog.show_main();
            }
        }
    });
}

pub fn refresh_repo(ui: &WindowUi, state: &AppState, app_name: &str) {
    let path_opt = state.current_path.borrow().clone();
    let branch_opt = state.current_branch.borrow().clone();
    if let Some(path) = path_opt {
        let started_at = Instant::now();
        ui.repo_view
            .commit_paging_state
            .borrow_mut()
            .pending_first_page_log = Some((
            started_at,
            path.clone(),
            "Refresh repo load -> rendered on screen".to_string(),
        ));
        load_repo(ui, state, app_name, path, branch_opt);
    }
}

pub fn close_repo(ui: &WindowUi, state: &AppState, app_name: &str) {
    state.clear_repo();
    ui.title_label.set_text(app_name);

    ui.repo_view.commit_list.clear();
    ui.repo_view.branch_panel.update_refs(&[], &[], None, None);

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
    ui.repo_view.branch_panel.update_refs(&[], &[], None, None);

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

pub fn maybe_load_repo_from_args(
    window: &gtk::ApplicationWindow,
    ui: &WindowUi,
    state: &AppState,
    app_name: &str,
) -> bool {
    let repo_arg: Option<PathBuf> = std::env::args_os().skip(1).find_map(|a| {
        // Ignore flags; treat the first non-flag arg as a path
        let s = a.to_string_lossy();
        if s.starts_with('-') {
            None
        } else {
            Some(PathBuf::from(a))
        }
    });

    if let Some(path) = repo_arg {
        if path.exists() {
            if let Err(e) = git::validate_repository(&path) {
                show_repo_error(window, &path, &e.to_string());
                ui.set_repo_controls_visible(false);
                ui.show_welcome();
                return false;
            }
            recent_repos::add_recent_repo(&path);
            ui.set_repo_controls_visible(true);
            ui.show_main();
            load_repo(ui, state, app_name, path, None);
            return true;
        }
    }

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
                recent_repos::add_recent_repo(&repo_root);
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
