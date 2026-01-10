use adw::prelude::AdwDialogExt;
use gio::ActionEntry;
use gtk::{gio, glib, prelude::*};

use super::repo;
use super::search;
use super::state::AppState;
use super::ui::WindowUi;
use crate::{APP_ID, DEVELOPER_NAME};

pub fn setup_shortcuts(app: &adw::Application) {
    app.set_accels_for_action("win.close", &["<Ctrl>W", "<Ctrl>Q"]);
    app.set_accels_for_action("win.open", &["<Ctrl>O"]);
    app.set_accels_for_action("win.close-repo", &["<Ctrl><Shift>W", "<Alt>Left"]);
    app.set_accels_for_action("win.show-help-overlay", &["<Ctrl>question"]);
    app.set_accels_for_action("win.show-search", &["<Ctrl>F"]);
    app.set_accels_for_action("win.hide-search", &["Escape"]);
    app.set_accels_for_action("win.find-next", &["<Ctrl>G"]);
    app.set_accels_for_action("win.find-previous", &["<Ctrl><Shift>G"]);
    app.set_accels_for_action("win.refresh", &["<Ctrl>R"]);
}

pub fn install(window: &gtk::ApplicationWindow, ui: &WindowUi, state: &AppState) {
    // Hook open button to the same open action (deduped open dialog)
    let open_button = ui.open_button.clone();
    let window_for_open_btn = window.clone();
    open_button.connect_clicked(move |_| {
        let _ = gtk::prelude::WidgetExt::activate_action(&window_for_open_btn, "win.open", None);
    });

    // Actions
    let action_close = ActionEntry::builder("close")
        .activate(|window: &gtk::ApplicationWindow, _, _| {
            window.close();
        })
        .build();

    let ui_for_open_action = ui.clone();
    let state_for_open_action = state.clone();
    let action_open = ActionEntry::builder("open")
        .activate(move |window: &gtk::ApplicationWindow, _, _| {
            repo::open_repo_dialog(
                window,
                &ui_for_open_action,
                &state_for_open_action,
                super::APP_NAME,
            );
        })
        .build();

    // Keyboard shortcuts overlay action
    let builder = gtk::Builder::from_resource("/com/markdeepwell/gity/gtk/shortcuts.ui");
    let shortcuts_window: gtk::ShortcutsWindow = builder
        .object("help_overlay")
        .expect("Could not get shortcuts window");

    let shortcuts_window_for_close = shortcuts_window.clone();
    shortcuts_window.connect_close_request(move |_| {
        shortcuts_window_for_close.set_visible(false);
        glib::Propagation::Stop
    });

    let shortcuts_window_for_esc = shortcuts_window.clone();
    let key_controller = gtk::EventControllerKey::new();
    key_controller.connect_key_pressed(move |_, keyval, _, _| {
        if keyval == gtk::gdk::Key::Escape {
            shortcuts_window_for_esc.set_visible(false);
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    shortcuts_window.add_controller(key_controller);

    let shortcuts_window_clone = shortcuts_window.clone();
    let action_show_help = ActionEntry::builder("show-help-overlay")
        .activate(move |window: &gtk::ApplicationWindow, _, _| {
            shortcuts_window_clone.set_transient_for(Some(window));
            shortcuts_window_clone.set_modal(true);
            shortcuts_window_clone.present();
        })
        .build();

    // Search actions + wiring
    let search_controller = search::SearchController::connect(ui, state);
    let action_show_search = search_controller.action_show_search(ui);
    let action_hide_search = search_controller.action_hide_search(ui);
    let action_find_next = search_controller.action_find_next(ui, state);
    let action_find_previous = search_controller.action_find_previous(ui, state);

    // Refresh action - created as SimpleAction so we can enable/disable it based on repo state
    let action_refresh = gio::SimpleAction::new("refresh", None);
    action_refresh.set_enabled(state.is_repo_loaded());
    let ui_for_refresh_action = ui.clone();
    let state_for_refresh_action = state.clone();
    action_refresh.connect_activate(move |_, _| {
        if state_for_refresh_action.is_repo_loaded() {
            repo::refresh_repo(
                &ui_for_refresh_action,
                &state_for_refresh_action,
                super::APP_NAME,
            );
        }
    });
    window.add_action(&action_refresh);
    ui.set_refresh_action(action_refresh);

    let ui_for_close_repo_action = ui.clone();
    let state_for_close_repo_action = state.clone();
    let action_close_repo = ActionEntry::builder("close-repo")
        .activate(move |_, _, _| {
            repo::close_repo(
                &ui_for_close_repo_action,
                &state_for_close_repo_action,
                super::APP_NAME,
            );
        })
        .build();

    // About window action
    let action_about = ActionEntry::builder("about")
        .activate(|window: &gtk::ApplicationWindow, _, _| {
            let about = adw::AboutDialog::builder()
                .application_name(super::APP_NAME)
                .application_icon(APP_ID)
                .developer_name(DEVELOPER_NAME)
                .version(env!("CARGO_PKG_VERSION"))
                .website("https://github.com/deepwell/gity")
                .issue_url("https://github.com/deepwell/gity/issues")
                .license_type(gtk::License::Gpl30)
                .build();
            about.present(Some(window));
        })
        .build();

    window.add_action_entries([
        action_close,
        action_open,
        action_show_help,
        action_show_search,
        action_hide_search,
        action_find_next,
        action_find_previous,
        action_close_repo,
        action_about,
    ]);
}
