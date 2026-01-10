use gtk::{gio, glib, prelude::*};

use crate::APP_ID;
mod actions;
mod diff;
pub mod recent_repos;
mod repo;
mod search;
mod state;
mod ui;

pub use actions::setup_shortcuts;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

const APP_NAME: &str = "GitY";

pub fn build_ui(application: &adw::Application) {
    // Load GSettings
    let settings = gio::Settings::new(APP_ID);

    let window = gtk::ApplicationWindow::builder()
        .default_width(1000)
        .default_height(1000)
        .application(application)
        .title(APP_NAME)
        .build();

    // Restore window state from GSettings
    let width = settings.int("window-width");
    let height = settings.int("window-height");
    let is_maximized = settings.boolean("is-maximized");

    window.set_default_size(width, height);
    if is_maximized {
        window.maximize();
    }
    // Build UI widgets (layout + stack views). Persistence wiring stays in this file.
    let ui = ui::WindowUi::build(window.clone(), APP_NAME);
    let app_state = state::AppState::new();

    // Branch selection reload (not an action).
    let current_path_for_branch = app_state.current_path.clone();
    let ui_for_branch_select = ui.clone();
    let state_for_branch_select = app_state.clone();
    ui.repo_view
        .branch_panel
        .branch_selected(move |branch_name| {
            // Avoid redundant reloads when selection changes programmatically.
            if state_for_branch_select.current_branch.borrow().as_deref() == Some(branch_name) {
                return;
            }
            let path_opt = current_path_for_branch.borrow().clone();
            if let Some(path) = path_opt {
                repo::load_repo(
                    &ui_for_branch_select,
                    &state_for_branch_select,
                    APP_NAME,
                    path,
                    Some(branch_name.to_string()),
                );
            }
        });

    // Wire actions + button handlers.
    actions::install(&window, &ui, &app_state);

    // Wire recent repository click handler
    let ui_for_recent = ui.clone();
    let state_for_recent = app_state.clone();
    ui.on_recent_repo_clicked(move |sandbox_path, real_path| {
        use std::time::Instant;

        // If a repository is already open, reset UI state first
        if state_for_recent.current_path.borrow().is_some() {
            repo::reset_for_repo_switch(&ui_for_recent, &state_for_recent);
        }

        let started_at = Instant::now();
        ui_for_recent
            .repo_view
            .commit_paging_state
            .borrow_mut()
            .pending_first_page_log = Some((
            started_at,
            sandbox_path.clone(),
            "Open repo load -> rendered on screen".to_string(),
        ));

        recent_repos::add_recent_repo(&sandbox_path, &real_path);
        repo::load_repo(
            &ui_for_recent,
            &state_for_recent,
            APP_NAME,
            sandbox_path,
            None,
        );
        ui_for_recent.set_repo_controls_visible(true);
        ui_for_recent.show_main();
    });

    // Wire recent repository removed handler to refresh the list
    let ui_for_removed = ui.clone();
    ui.on_recent_repo_removed(move || {
        ui_for_removed.refresh_recent_repos();
    });

    // Refresh recent repos on welcome screen
    ui.refresh_recent_repos();

    // Hook diff loader to commit selection changes
    diff::connect(&ui, &app_state);

    // Create paned widget to allow resizing between commits list and diff view
    let main_content_paned = ui.repo_view.main_content_paned.clone();

    // Restore paned position from GSettings
    let paned_position = settings.int("diff-paned-position");
    main_content_paned.set_position(paned_position);

    // Save paned position when it changes
    let settings_for_paned = settings.clone();
    main_content_paned.connect_position_notify(move |paned| {
        let _ = settings_for_paned.set_int("diff-paned-position", paned.position());
    });

    // Create horizontal paned to hold side panel and main content (allows resizing)
    let horizontal_paned = ui.repo_view.horizontal_paned.clone();

    // Restore branch panel width from GSettings
    let branch_panel_width = settings.int("branch-panel-width");
    horizontal_paned.set_position(branch_panel_width);

    // Save branch panel width when it changes
    let settings_for_branch_panel = settings.clone();
    horizontal_paned.connect_position_notify(move |paned| {
        let _ = settings_for_branch_panel.set_int("branch-panel-width", paned.position());
    });

    // Main/welcome views are added by ui::WindowUi::build().

    // Save window state to GSettings when window is resized
    // Use an atomic bool to track if we're currently restoring to avoid saving during restore
    let restoring = Arc::new(AtomicBool::new(true));

    // Helper to save window state
    let save_state =
        |win: &gtk::ApplicationWindow, set: &gio::Settings, restoring_flag: &Arc<AtomicBool>| {
            if !restoring_flag.load(Ordering::Relaxed) {
                let _ = set.set_boolean("is-maximized", win.is_maximized());
                if !win.is_maximized() {
                    let (width, height) = win.default_size();
                    let _ = set.set_int("window-width", width);
                    let _ = set.set_int("window-height", height);
                }
            }
        };

    // Save state when window is closed
    let settings_close = settings.clone();
    let restoring_close = restoring.clone();
    window.connect_close_request(move |win| {
        save_state(win, &settings_close, &restoring_close);
        glib::Propagation::Proceed
    });

    // Save state periodically using a recurring timeout (every 500ms)
    let settings_timeout = settings.clone();
    let window_timeout = window.clone();
    let restoring_timeout = restoring.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(500), move || {
        save_state(&window_timeout, &settings_timeout, &restoring_timeout);
        glib::ControlFlow::Continue
    });

    // Save maximized state immediately when it changes
    let settings_max = settings.clone();
    let restoring_max = restoring.clone();
    window.connect_maximized_notify(move |win| {
        if !restoring_max.load(Ordering::Relaxed) {
            let _ = settings_max.set_boolean("is-maximized", win.is_maximized());
        }
    });

    // Mark that we're done restoring after the window is shown
    let restoring_realize = restoring.clone();
    window.connect_realize(move |_| {
        restoring_realize.store(false, Ordering::Relaxed);
    });

    // Decide initial screen / optional CLI repo load.
    // If validation fails, we still present the window (to show the error + welcome UI).
    let _loaded = repo::maybe_load_repo_from_args(&window, &ui, &app_state, APP_NAME);

    window.present();
}
