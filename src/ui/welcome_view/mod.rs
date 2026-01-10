//! Welcome view component displayed when no repository is open.
//!
//! This module encapsulates the welcome screen UI including:
//! - Welcome message and open button
//! - Recent repositories list with cards

use gtk::{gio, prelude::*};
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::window::recent_repos::{self, RecentRepo};

/// Callback type for when a recent repository is clicked
/// Receives (sandbox_path, real_path)
pub type RepoClickedCallback = Rc<RefCell<Option<Box<dyn Fn(PathBuf, PathBuf)>>>>;
/// Callback type for when a recent repository is removed (to trigger refresh)
pub type RepoRemovedCallback = Rc<RefCell<Option<Box<dyn Fn()>>>>;

/// The welcome view shown when no repository is loaded.
#[derive(Clone)]
pub struct WelcomeView {
    /// Root widget containing the entire welcome view
    pub widget: gtk::Box,
    /// Container for recent repository cards
    recent_repos_container: gtk::Box,
    /// Callback invoked when a repo card is clicked
    repo_clicked_callback: RepoClickedCallback,
    /// Callback invoked when a repo is removed from the list
    repo_removed_callback: RepoRemovedCallback,
}

impl WelcomeView {
    /// Create a new WelcomeView.
    ///
    /// # Arguments
    /// * `window` - The parent application window (for action dispatch)
    pub fn new(window: &gtk::ApplicationWindow) -> Self {
        let welcome_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .valign(gtk::Align::Center)
            .halign(gtk::Align::Center)
            .spacing(20)
            .margin_top(40)
            .margin_bottom(40)
            .margin_start(40)
            .margin_end(40)
            .build();

        let welcome_title = gtk::Label::builder()
            .label("Open a Git repository")
            .halign(gtk::Align::Center)
            .build();
        welcome_title.add_css_class("title-1");

        let welcome_subtitle = gtk::Label::builder()
            .label("Choose a folder containing a Git repository to get started.")
            .halign(gtk::Align::Center)
            .wrap(true)
            .build();
        welcome_subtitle.add_css_class("dim-label");

        let welcome_open_button = gtk::Button::builder()
            .label("Open Repository…")
            .halign(gtk::Align::Center)
            .build();
        welcome_open_button.add_css_class("suggested-action");

        welcome_box.append(&welcome_title);
        welcome_box.append(&welcome_subtitle);
        welcome_box.append(&welcome_open_button);

        // Container for recent repositories
        let recent_repos_container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(8)
            .halign(gtk::Align::Center)
            .margin_top(16)
            .build();

        welcome_box.append(&recent_repos_container);

        // Callbacks for recent repo actions
        let repo_clicked_callback: RepoClickedCallback = Rc::new(RefCell::new(None));
        let repo_removed_callback: RepoRemovedCallback = Rc::new(RefCell::new(None));

        // Wire open button to the window's open action
        let window_for_open = window.clone();
        welcome_open_button.connect_clicked(move |_| {
            let _ = gtk::prelude::WidgetExt::activate_action(&window_for_open, "win.open", None);
        });

        Self {
            widget: welcome_box,
            recent_repos_container,
            repo_clicked_callback,
            repo_removed_callback,
        }
    }

    /// Set the callback invoked when a recent repository card is clicked.
    /// The callback receives (sandbox_path, real_path).
    pub fn on_repo_clicked<F: Fn(PathBuf, PathBuf) + 'static>(&self, callback: F) {
        *self.repo_clicked_callback.borrow_mut() = Some(Box::new(callback));
    }

    /// Set the callback invoked when a recent repository is removed.
    pub fn on_repo_removed<F: Fn() + 'static>(&self, callback: F) {
        *self.repo_removed_callback.borrow_mut() = Some(Box::new(callback));
    }

    /// Refresh the recent repositories list.
    ///
    /// This reloads repos from settings and rebuilds the UI cards.
    pub fn refresh_recent_repos(&self) {
        // Clear existing children
        while let Some(child) = self.recent_repos_container.first_child() {
            self.recent_repos_container.remove(&child);
        }

        let recent_repos = recent_repos::load_recent_repos();
        if recent_repos.is_empty() {
            return;
        }

        // Cap the number of columns by item count (1..4), while still allowing
        // the layout to wrap down on smaller window widths.
        let max_cols: u32 = (recent_repos.len().min(4)).max(1) as u32;

        // Add "Recent Repositories" header
        let header = gtk::Label::builder()
            .label("Recent Repositories")
            .halign(gtk::Align::Center)
            .margin_top(12)
            .build();
        header.add_css_class("title-4");
        self.recent_repos_container.append(&header);

        // Create a flow box for the repo cards
        let flow_box = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .activate_on_single_click(false)
            .homogeneous(true)
            .max_children_per_line(max_cols)
            .min_children_per_line(1)
            .column_spacing(12)
            .row_spacing(12)
            .halign(gtk::Align::Center)
            .build();

        // Handle Enter key activation on FlowBox children
        // Note: For FlowBox activation, we'll need to store both paths in the widget name
        // For now, we'll use a JSON format: {"sandbox":"...","real":"..."}
        let callback_for_activate = self.repo_clicked_callback.clone();
        flow_box.connect_child_activated(move |_, child| {
            let name = child.child().map(|c| c.widget_name());
            if let Some(name) = name {
                if !name.is_empty() {
                    // Try to parse as JSON first (new format)
                    if let Ok(paths) = serde_json::from_str::<serde_json::Value>(&name) {
                        if let (Some(sandbox), Some(real)) = (
                            paths.get("sandbox").and_then(|v| v.as_str()),
                            paths.get("real").and_then(|v| v.as_str()),
                        ) {
                            if let Some(ref cb) = *callback_for_activate.borrow() {
                                cb(PathBuf::from(sandbox), PathBuf::from(real));
                            }
                            return;
                        }
                    }
                    // Fallback: treat as single path (old format or non-sandbox)
                    let path = PathBuf::from(name.as_str());
                    if let Some(ref cb) = *callback_for_activate.borrow() {
                        cb(path.clone(), path);
                    }
                }
            }
        });

        for repo in recent_repos {
            let card = Self::build_repo_card(&repo);

            // Store both paths in widget name as JSON for FlowBox activation handler
            let paths_json = serde_json::json!({
                "sandbox": repo.path.to_string_lossy(),
                "real": repo.real_path.to_string_lossy()
            });
            card.set_widget_name(&paths_json.to_string());

            // Set up left-click handler to open repo
            let sandbox_path = repo.path.clone();
            let real_path = repo.real_path.clone();
            let callback = self.repo_clicked_callback.clone();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(1);
            gesture.connect_pressed(move |_, _, _, _| {
                if let Some(ref cb) = *callback.borrow() {
                    cb(sandbox_path.clone(), real_path.clone());
                }
            });
            card.add_controller(gesture);

            // Set up right-click context menu to remove repo
            let path_for_menu = repo.path.clone();
            let right_click = gtk::GestureClick::new();
            right_click.set_button(3);

            // Create popover menu
            let popover = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
            popover.set_parent(&card);
            popover.set_has_arrow(true);

            // Create menu model
            let menu = gio::Menu::new();
            menu.append(Some("Remove from list"), Some("recent.remove"));
            popover.set_menu_model(Some(&menu));

            // Create action group for the remove action
            let action_group = gio::SimpleActionGroup::new();
            let remove_action = gio::SimpleAction::new("remove", None);

            let path_for_action = path_for_menu.clone();
            let removed_callback = self.repo_removed_callback.clone();
            remove_action.connect_activate(move |_, _| {
                recent_repos::remove_recent_repo(&path_for_action);
                // Trigger refresh callback
                if let Some(ref cb) = *removed_callback.borrow() {
                    cb();
                }
            });
            action_group.add_action(&remove_action);
            card.insert_action_group("recent", Some(&action_group));

            let popover_for_click = popover.clone();
            right_click.connect_pressed(move |_, _, x, y| {
                popover_for_click
                    .set_pointing_to(Some(&gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                popover_for_click.popup();
            });
            card.add_controller(right_click);

            flow_box.append(&card);
        }

        self.recent_repos_container.append(&flow_box);
    }

    /// Build a single repository card widget.
    fn build_repo_card(repo: &RecentRepo) -> gtk::Box {
        let card = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .width_request(280)
            .build();
        card.add_css_class("card");
        card.add_css_class("repo-card");

        // Make the card feel clickable
        card.set_cursor_from_name(Some("pointer"));

        // Inner padding box
        let inner = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_top(16)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();

        // Folder name (large)
        let folder_label = gtk::Label::builder()
            .label(&repo.folder_name)
            .halign(gtk::Align::Start)
            .single_line_mode(true)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(40)
            .build();
        folder_label.add_css_class("title-3");

        // Full path (smaller, dimmed)
        let path_label = gtk::Label::builder()
            .label(&repo.display_path)
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::Middle)
            .max_width_chars(40)
            .build();
        path_label.add_css_class("caption");
        path_label.add_css_class("dim-label");

        inner.append(&folder_label);
        inner.append(&path_label);
        card.append(&inner);

        card
    }
}
