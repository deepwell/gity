use gtk::{gio, prelude::*};

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::ui::{RepoView, WelcomeView};

#[derive(Clone)]
pub struct WindowUi {
    // Header / navigation
    pub title_label: gtk::Label,
    pub close_repo_button: gtk::Button,
    pub open_button: gtk::Button,
    pub search_button: gtk::Button,

    // Root navigation stack
    pub stack: gtk::Stack,

    // Repository screen (search + panels + diff)
    pub repo_view: RepoView,

    // Welcome screen (new component-based approach)
    welcome_view: WelcomeView,

    // Action that requires a repository to be loaded
    refresh_action: Rc<RefCell<Option<gio::SimpleAction>>>,
}

impl WindowUi {
    pub fn build(window: gtk::ApplicationWindow, app_name: &str) -> Self {
        // App-wide CSS tweaks used by multiple screens.
        crate::ui::styles::install();

        // Header bar + title
        let header_bar = adw::HeaderBar::new();

        let title_label = gtk::Label::builder().label(app_name).build();
        header_bar.set_title_widget(Some(&title_label));

        let close_repo_button = gtk::Button::builder()
            .icon_name("go-previous-symbolic")
            .tooltip_text("Back to Welcome Screen")
            .visible(false)
            .build();
        header_bar.pack_start(&close_repo_button);

        let open_button = gtk::Button::builder()
            .icon_name("document-open-symbolic")
            .tooltip_text("Open Git Repository")
            .build();
        header_bar.pack_start(&open_button);

        let search_button = gtk::Button::builder()
            .icon_name("system-search-symbolic")
            .tooltip_text("Search")
            .build();
        header_bar.pack_start(&search_button);

        // Header bar menu (overflow / hamburger)
        let menu_button = gtk::MenuButton::builder()
            .icon_name("open-menu-symbolic")
            .tooltip_text("Menu")
            .build();
        let menu = gio::Menu::new();
        let menu_section = gio::Menu::new();
        menu_section.append(Some("Reload Repository"), Some("win.refresh"));
        menu_section.append(Some("Keyboard Shortcuts"), Some("win.show-help-overlay"));
        menu_section.append(Some(&format!("About {}", app_name)), Some("win.about"));
        menu.append_section(None, &menu_section);
        menu_button.set_menu_model(Some(&menu));
        header_bar.pack_end(&menu_button);

        let window_for_search_btn = window.clone();
        search_button.connect_clicked(move |_| {
            let _ = gtk::prelude::WidgetExt::activate_action(
                &window_for_search_btn,
                "win.show-search",
                None,
            );
        });

        let window_for_close_repo_btn = window.clone();
        close_repo_button.connect_clicked(move |_| {
            let _ = gtk::prelude::WidgetExt::activate_action(
                &window_for_close_repo_btn,
                "win.close-repo",
                None,
            );
        });

        window.set_titlebar(Some(&header_bar));

        // Root stack holds either welcome or main UI
        let stack = gtk::Stack::builder().hexpand(true).vexpand(true).build();
        window.set_child(Some(&stack));

        // Repository view (search + panels + diff)
        let repo_view = RepoView::new(&window);
        stack.add_named(&repo_view.widget, Some("main"));

        // Welcome view (using the new component)
        let welcome_view = WelcomeView::new(&window);
        stack.add_named(&welcome_view.widget, Some("welcome"));

        Self {
            title_label,
            close_repo_button,
            open_button,
            search_button,
            stack,
            repo_view,
            welcome_view,
            refresh_action: Rc::new(RefCell::new(None)),
        }
    }

    /// Store the refresh action so we can enable/disable it based on repo state.
    pub fn set_refresh_action(&self, action: gio::SimpleAction) {
        *self.refresh_action.borrow_mut() = Some(action);
    }

    /// Set a callback for when a recent repository card is clicked.
    /// The callback receives (sandbox_path, real_path).
    pub fn on_recent_repo_clicked<F: Fn(PathBuf, PathBuf) + 'static>(&self, callback: F) {
        self.welcome_view.on_repo_clicked(callback);
    }

    /// Set a callback for when a recent repository is removed (to refresh the list).
    pub fn on_recent_repo_removed<F: Fn() + 'static>(&self, callback: F) {
        self.welcome_view.on_repo_removed(callback);
    }

    /// Refresh the recent repositories list on the welcome screen.
    pub fn refresh_recent_repos(&self) {
        self.welcome_view.refresh_recent_repos();
    }

    pub fn set_repo_controls_visible(&self, visible: bool) {
        self.search_button.set_visible(visible);
        self.close_repo_button.set_visible(visible);

        // Enable/disable the refresh action based on whether a repo is loaded
        if let Some(ref action) = *self.refresh_action.borrow() {
            action.set_enabled(visible);
        }
    }

    pub fn show_main(&self) {
        self.stack.set_visible_child_name("main");
    }

    pub fn show_welcome(&self) {
        self.stack.set_visible_child_name("welcome");
    }
}
