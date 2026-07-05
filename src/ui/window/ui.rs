use gtk::{gio, prelude::*};

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::ui::{RepoView, WelcomeView};

#[derive(Clone)]
pub struct WindowUi {
    // The top-level window (used to keep the OS window title in sync).
    window: gtk::ApplicationWindow,
    // The application name, used when composing the OS window title.
    app_name: String,

    // Header / navigation
    pub title: adw::WindowTitle,
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

        // adw::WindowTitle shows the repository name as the title and additional
        // context (e.g. the current branch) as the subtitle, following the
        // standard Adwaita header-bar pattern.
        let title = adw::WindowTitle::new(app_name, "");
        header_bar.set_title_widget(Some(&title));

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
            window,
            app_name: app_name.to_string(),
            title,
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

    /// Set the header title to the repository name with a subtitle (typically
    /// the current branch/ref). Also updates the OS window title (taskbar,
    /// alt-tab, overview) to "repo - branch - AppName".
    pub fn set_repo_title(&self, repo_name: &str, subtitle: &str) {
        self.title.set_title(repo_name);
        self.title.set_subtitle(subtitle);
        self.update_os_title(repo_name, subtitle);
    }

    /// Update only the header subtitle (e.g. after switching branch or tag).
    /// Keeps the OS window title in sync with the new branch/ref.
    pub fn set_repo_subtitle(&self, subtitle: &str) {
        self.title.set_subtitle(subtitle);
        let repo_name = self.title.title();
        self.update_os_title(repo_name.as_str(), subtitle);
    }

    /// Reset the header and OS window titles back to the application name with
    /// no subtitle.
    pub fn reset_title(&self, app_name: &str) {
        self.title.set_title(app_name);
        self.title.set_subtitle("");
        self.window.set_title(Some(app_name));
    }

    /// Compose and apply the OS window title as "repo - branch - AppName"
    /// (omitting the branch when it is empty).
    fn update_os_title(&self, repo_name: &str, branch: &str) {
        let os_title = if branch.is_empty() {
            format!("{} - {}", repo_name, self.app_name)
        } else {
            format!("{} - {} - {}", repo_name, branch, self.app_name)
        };
        self.window.set_title(Some(&os_title));
    }

    pub fn show_main(&self) {
        self.stack.set_visible_child_name("main");
    }

    pub fn show_welcome(&self) {
        self.stack.set_visible_child_name("welcome");
    }
}
