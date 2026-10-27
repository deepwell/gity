//! Branch panel UI component for displaying and selecting git branches and tags.
//!
//! This module provides the `BranchPanel` widget which displays a list of
//! git branches, tags, stashes, and remote-tracking branches with their last commit time and
//! allows single-selection. All sections are collapsible with state persisted
//! to gsettings. A filter bar (hidden until requested) narrows every section
//! to the refs matching the typed text.

use chrono::{DateTime, Utc};
use gtk::{gio, prelude::*};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;

use crate::APP_ID;
use crate::git::{BranchInfo, StashInfo, TagInfo};

/// Type of git reference (branch, remote branch, tag, or stash).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefType {
    Branch,
    Remote,
    Tag,
    Stash,
}

/// A create or delete request raised from the panel's header buttons and
/// row context menus (see `on_ref_action`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefAction {
    NewBranch,
    NewTag,
    Delete { name: String, ref_type: RefType },
}

/// Information about the currently selected reference.
#[derive(Clone)]
struct SelectedRef {
    name: String,
    ref_type: RefType,
}

/// A panel widget that displays local branches, tags, and remote-tracking branches.
///
/// The panel shows refs in separate collapsible sections:
/// - **Branches** — local branches with a checkmark on the checked-out branch
/// - **Tags**
/// - **Stashes** — hidden when the repository has no stashes
/// - **Remotes** — grouped by remote name (e.g. `origin`), each with its own expander
///
/// Branches are sorted with "main" or "master" first, then by latest commit time.
/// Tags are sorted alphabetically using natural sort order.
///
/// Expanded/collapsed state is persisted to gsettings.
///
/// A filter bar above the sections (see `show_filter`) hides the refs that do
/// not match the typed text.
#[derive(Clone)]
pub struct BranchPanel {
    /// The root widget container
    pub widget: gtk::Box,
    /// The search bar holding the filter entry (hidden until `show_filter`)
    filter_bar: gtk::SearchBar,
    /// The entry whose text filters the ref rows
    filter_entry: gtk::SearchEntry,
    /// Lowercased filter text, shared with each list box's filter function
    filter_query: Rc<RefCell<String>>,
    /// Shown instead of the sections when the filter matches no refs
    no_matches_label: gtk::Label,
    /// The list box containing branch rows
    branches_list_box: gtk::ListBox,
    /// The list box containing tag rows
    tags_list_box: gtk::ListBox,
    /// The list box containing stash rows
    stashes_list_box: gtk::ListBox,
    /// Container for per-remote expanders inside the Remotes section
    remotes_content_box: gtk::Box,
    /// List boxes for remote branch rows (one per remote, rebuilt on refresh)
    remote_list_boxes: Rc<RefCell<Vec<gtk::ListBox>>>,
    /// The expander for branches section
    branches_expander: gtk::Expander,
    /// The expander for tags section
    tags_expander: gtk::Expander,
    /// The expander for stashes section (hidden when there are no stashes)
    stashes_expander: gtk::Expander,
    /// The expander for remotes section
    remotes_expander: gtk::Expander,
    /// Section labels used to show ref counts on hover
    branches_label: gtk::Label,
    tags_label: gtk::Label,
    stashes_label: gtk::Label,
    remotes_label: gtk::Label,
    /// Handler invoked when a ref row is activated (set via `on_ref_selected`)
    activate_handler: Rc<RefCell<Option<Rc<dyn Fn(&str, RefType)>>>>,
    /// Handler invoked for create/delete requests (set via `on_ref_action`)
    ref_action_handler: Rc<RefCell<Option<Rc<dyn Fn(RefAction)>>>>,
    /// Name of the checked-out branch, which can't be deleted
    checked_out_branch: Rc<RefCell<Option<String>>>,
    /// Currently selected reference (name and type)
    selected_ref: Rc<RefCell<Option<SelectedRef>>>,
    /// GSettings for persisting expanded state
    settings: gio::Settings,
}

impl BranchPanel {
    /// Create a new BranchPanel with the given branches.
    ///
    /// # Arguments
    /// * `branches` - Slice of branch information to display
    pub fn new(branches: &[BranchInfo]) -> Self {
        Self::new_with_refs(branches, &[], &[], &[], None, None)
    }

    /// Create a new BranchPanel with branches, remote branches, tags, and indication of current state.
    ///
    /// # Arguments
    /// * `branches` - Slice of local branch information to display
    /// * `remote_branches` - Slice of remote-tracking branch information to display
    /// * `tags` - Slice of tag information to display
    /// * `stashes` - Slice of stash entries to display
    /// * `checked_out_branch` - Name of the currently checked out branch (if any)
    /// * `current_ref_name` - Name of the currently viewed ref (branch or tag)
    pub fn new_with_refs(
        branches: &[BranchInfo],
        remote_branches: &[BranchInfo],
        tags: &[TagInfo],
        stashes: &[StashInfo],
        checked_out_branch: Option<&str>,
        current_ref_name: Option<&str>,
    ) -> Self {
        let settings = gio::Settings::new(APP_ID);

        let side_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["branch-panel"])
            .build();
        side_panel.set_width_request(150);

        // Filter bar, revealed by `show_filter`. Connecting the entry lets the
        // bar close itself on Escape and clear the text when it closes.
        let filter_entry = gtk::SearchEntry::builder()
            .placeholder_text("Filter by name")
            .hexpand(true)
            .build();
        let filter_bar = gtk::SearchBar::builder().child(&filter_entry).build();
        filter_bar.connect_entry(&filter_entry);
        side_panel.append(&filter_bar);
        let filter_query = Rc::new(RefCell::new(String::new()));

        // Create a scrolled window for the content
        let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();

        // Create container for expanders
        let content_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        let no_matches_label = gtk::Label::builder()
            .label("No matching branches, tags, or stashes")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .margin_top(24)
            .margin_start(12)
            .margin_end(12)
            .visible(false)
            .build();
        no_matches_label.add_css_class("dim-label");
        content_box.append(&no_matches_label);

        // Create branches section
        let branches_expander = gtk::Expander::builder()
            .expanded(settings.boolean("branches-expanded"))
            .build();
        branches_expander.add_css_class("branch-panel-expander");

        let branches_label = gtk::Label::builder()
            .label("Branches")
            .halign(gtk::Align::Start)
            .build();
        branches_label.add_css_class("heading");
        let branches_label_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        branches_label_box.add_css_class("branch-panel-expander-label");
        branches_label_box.append(&branches_label);
        append_header_button(&branches_label_box, "New Branch", "refs.new-branch");
        branches_expander.set_label_widget(Some(&branches_label_box));
        set_expander_chevron_margin(&branches_expander, 10);

        let branches_list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();

        set_ref_filter_func(&branches_list_box, &filter_query);

        branches_expander.set_child(Some(&branches_list_box));
        content_box.append(&branches_expander);

        // Create tags section
        let tags_expander = gtk::Expander::builder()
            .expanded(settings.boolean("tags-expanded"))
            .build();
        tags_expander.add_css_class("branch-panel-expander");

        let tags_label = gtk::Label::builder()
            .label("Tags")
            .halign(gtk::Align::Start)
            .build();
        tags_label.add_css_class("heading");
        let tags_label_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        tags_label_box.add_css_class("branch-panel-expander-label");
        tags_label_box.append(&tags_label);
        append_header_button(&tags_label_box, "New Tag", "refs.new-tag");
        tags_expander.set_label_widget(Some(&tags_label_box));
        set_expander_chevron_margin(&tags_expander, 10);

        let tags_list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();

        set_ref_filter_func(&tags_list_box, &filter_query);

        tags_expander.set_child(Some(&tags_list_box));
        content_box.append(&tags_expander);

        // Create stashes section
        let stashes_expander = gtk::Expander::builder()
            .expanded(settings.boolean("stashes-expanded"))
            .build();
        stashes_expander.add_css_class("branch-panel-expander");

        let stashes_label = gtk::Label::builder()
            .label("Stashes")
            .halign(gtk::Align::Start)
            .build();
        stashes_label.add_css_class("heading");
        let stashes_label_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        stashes_label_box.add_css_class("branch-panel-expander-label");
        stashes_label_box.append(&stashes_label);
        stashes_expander.set_label_widget(Some(&stashes_label_box));
        set_expander_chevron_margin(&stashes_expander, 10);

        let stashes_list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();

        set_ref_filter_func(&stashes_list_box, &filter_query);

        stashes_expander.set_child(Some(&stashes_list_box));
        content_box.append(&stashes_expander);

        // Create remotes section (after branches and tags)
        let remotes_expander = gtk::Expander::builder()
            .expanded(settings.boolean("remotes-expanded"))
            .build();
        remotes_expander.add_css_class("branch-panel-expander");

        let remotes_label = gtk::Label::builder()
            .label("Remotes")
            .halign(gtk::Align::Start)
            .build();
        remotes_label.add_css_class("heading");
        let remotes_label_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        remotes_label_box.add_css_class("branch-panel-expander-label");
        remotes_label_box.append(&remotes_label);
        remotes_expander.set_label_widget(Some(&remotes_label_box));
        set_expander_chevron_margin(&remotes_expander, 10);

        let remotes_content_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        remotes_expander.set_child(Some(&remotes_content_box));
        content_box.append(&remotes_expander);

        let remote_list_boxes = Rc::new(RefCell::new(Vec::new()));
        let activate_handler: Rc<RefCell<Option<Rc<dyn Fn(&str, RefType)>>>> =
            Rc::new(RefCell::new(None));
        let selected_ref = Rc::new(RefCell::new(None));
        let ref_action_handler: Rc<RefCell<Option<Rc<dyn Fn(RefAction)>>>> =
            Rc::new(RefCell::new(None));
        let checked_out_branch_name = Rc::new(RefCell::new(checked_out_branch.map(String::from)));

        // Create/delete actions, shared by the header buttons and the row
        // context menus. Remote branches have no menu.
        let context_ref = Rc::new(RefCell::new(None));
        let delete_action = install_ref_actions(&side_panel, &ref_action_handler, &context_ref);
        let ref_menu = gio::Menu::new();
        let create_section = gio::Menu::new();
        create_section.append(Some("New Branch…"), Some("refs.new-branch"));
        create_section.append(Some("New Tag…"), Some("refs.new-tag"));
        ref_menu.append_section(None, &create_section);
        let delete_section = gio::Menu::new();
        delete_section.append(Some("Delete…"), Some("refs.delete"));
        ref_menu.append_section(None, &delete_section);
        let stash_menu = gio::Menu::new();
        stash_menu.append(Some("Delete…"), Some("refs.delete"));
        // One popover for all sections, parented to the panel's root box.
        // A list box can't hold it: it may only contain rows.
        let context_popover = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
        context_popover.set_parent(&side_panel);
        context_popover.set_has_arrow(true);
        for (list_box, menu) in [
            (&branches_list_box, &ref_menu),
            (&tags_list_box, &ref_menu),
            (&stashes_list_box, &stash_menu),
        ] {
            attach_context_menu(
                list_box,
                menu,
                &context_popover,
                &context_ref,
                &checked_out_branch_name,
                &delete_action,
            );
        }

        scrolled.set_child(Some(&content_box));
        side_panel.append(&scrolled);
        side_panel.set_vexpand(true);

        populate_branches_list(&branches_list_box, branches, checked_out_branch);
        populate_tags_list(&tags_list_box, tags);
        populate_stashes_list(&stashes_expander, &stashes_list_box, stashes);
        populate_remotes_section(
            &remotes_content_box,
            &remote_list_boxes,
            &[&branches_list_box, &tags_list_box, &stashes_list_box],
            &selected_ref,
            &activate_handler,
            &filter_query,
            remote_branches,
        );
        set_section_count_tooltips(
            &branches_label,
            &tags_label,
            &stashes_label,
            &remotes_label,
            branches,
            remote_branches,
            tags,
            stashes,
        );

        // Wire up settings persistence for expanded state. Filtering expands
        // every section temporarily, which must not overwrite the saved state.
        for (expander, key) in [
            (&branches_expander, "branches-expanded"),
            (&tags_expander, "tags-expanded"),
            (&stashes_expander, "stashes-expanded"),
            (&remotes_expander, "remotes-expanded"),
        ] {
            let settings = settings.clone();
            let filter_query = filter_query.clone();
            expander.connect_expanded_notify(move |exp| {
                if filter_query.borrow().is_empty() {
                    let _ = settings.set_boolean(key, exp.is_expanded());
                }
            });
        }

        wire_local_list_selection(
            &[&branches_list_box, &tags_list_box, &stashes_list_box],
            &remote_list_boxes,
            &selected_ref,
        );

        let panel = Self {
            widget: side_panel,
            filter_bar,
            filter_entry,
            filter_query,
            no_matches_label,
            branches_list_box,
            tags_list_box,
            stashes_list_box,
            remotes_content_box,
            remote_list_boxes,
            branches_expander,
            tags_expander,
            stashes_expander,
            remotes_expander,
            branches_label,
            tags_label,
            stashes_label,
            remotes_label,
            activate_handler,
            ref_action_handler,
            checked_out_branch: checked_out_branch_name,
            selected_ref,
            settings,
        };

        let panel_for_changed = panel.clone();
        panel
            .filter_entry
            .connect_search_changed(move |_| panel_for_changed.apply_filter());

        // Enter jumps to the first matching ref.
        let panel_for_activate = panel.clone();
        panel
            .filter_entry
            .connect_activate(move |_| panel_for_activate.activate_first_match());

        // Closing the bar drops the filter so every ref is listed again.
        let panel_for_close = panel.clone();
        panel
            .filter_bar
            .connect_search_mode_enabled_notify(move |bar| {
                if !bar.is_search_mode() {
                    panel_for_close.filter_entry.set_text("");
                    panel_for_close.apply_filter();
                }
            });

        // Default selection: prefer current ref, then checked-out branch, then "main", then first branch.
        if let Some(name) = current_ref_name {
            let _ = panel.select_ref(name);
        } else if let Some(name) = checked_out_branch {
            let _ = panel.select_ref(name);
        }
        panel.ensure_default_selection();
        panel
    }

    /// Update the panel with new lists of branches, remote branches, tags, and stashes.
    ///
    /// Attempts to preserve the current selection if possible.
    pub fn update_refs(
        &self,
        branches: &[BranchInfo],
        remote_branches: &[BranchInfo],
        tags: &[TagInfo],
        stashes: &[StashInfo],
        checked_out_branch: Option<&str>,
        current_ref_name: Option<&str>,
    ) {
        // Preserve the last selected ref (or current selected row if present).
        let preserved = self
            .selected_ref
            .borrow()
            .clone()
            .or_else(|| self.selected_row_ref_info());

        while let Some(row) = self.branches_list_box.row_at_index(0) {
            self.branches_list_box.remove(&row);
        }

        while let Some(row) = self.tags_list_box.row_at_index(0) {
            self.tags_list_box.remove(&row);
        }

        while let Some(row) = self.stashes_list_box.row_at_index(0) {
            self.stashes_list_box.remove(&row);
        }

        clear_remotes_section(&self.remotes_content_box, &self.remote_list_boxes);

        *self.checked_out_branch.borrow_mut() = checked_out_branch.map(String::from);

        populate_branches_list(&self.branches_list_box, branches, checked_out_branch);
        populate_tags_list(&self.tags_list_box, tags);
        populate_stashes_list(&self.stashes_expander, &self.stashes_list_box, stashes);
        populate_remotes_section(
            &self.remotes_content_box,
            &self.remote_list_boxes,
            &[
                &self.branches_list_box,
                &self.tags_list_box,
                &self.stashes_list_box,
            ],
            &self.selected_ref,
            &self.activate_handler,
            &self.filter_query,
            remote_branches,
        );
        set_section_count_tooltips(
            &self.branches_label,
            &self.tags_label,
            &self.stashes_label,
            &self.remotes_label,
            branches,
            remote_branches,
            tags,
            stashes,
        );

        if let Some(ref_info) = preserved {
            let _ = self.select_ref(&ref_info.name);
        } else if let Some(name) = current_ref_name {
            let _ = self.select_ref(name);
        } else if let Some(name) = checked_out_branch {
            let _ = self.select_ref(name);
        }
        self.ensure_default_selection();

        // The rows were rebuilt, so section visibility must be recomputed.
        self.apply_filter();
    }

    /// Reveal the filter bar and focus its entry.
    pub fn show_filter(&self) {
        self.filter_bar.set_search_mode(true);
        self.filter_entry.grab_focus();
    }

    /// Hide the filter bar and list every ref again.
    pub fn hide_filter(&self) {
        self.filter_bar.set_search_mode(false);
    }

    /// Whether keyboard focus is in the filter entry.
    pub fn filter_has_focus(&self) -> bool {
        self.filter_entry
            .state_flags()
            .contains(gtk::StateFlags::FOCUS_WITHIN)
    }

    /// Apply the filter entry's text to every section.
    ///
    /// While a filter is active, sections with matches are expanded (so a
    /// collapsed section can't hide them) and sections without are hidden.
    fn apply_filter(&self) {
        let query = self.filter_entry.text().trim().to_lowercase();
        let filtering = !query.is_empty();
        *self.filter_query.borrow_mut() = query.clone();

        // Re-run the row filter, then report whether any row is still listed.
        let refilter = |list_box: &gtk::ListBox| {
            list_box.invalidate_filter();
            list_box_rows(list_box).any(|row| row_matches_filter(&row, &query))
        };

        // The stashes section is the only one hidden when it has no rows at all.
        let mut any_match = false;
        for (expander, key, list_box, hide_when_empty) in [
            (
                &self.branches_expander,
                "branches-expanded",
                &self.branches_list_box,
                false,
            ),
            (
                &self.tags_expander,
                "tags-expanded",
                &self.tags_list_box,
                false,
            ),
            (
                &self.stashes_expander,
                "stashes-expanded",
                &self.stashes_list_box,
                true,
            ),
        ] {
            let matched = refilter(list_box);
            any_match |= matched;
            expander.set_visible(matched || !(filtering || hide_when_empty));
            expander.set_expanded(filtering || self.settings.boolean(key));
        }

        let mut any_remote_match = false;
        for list_box in self.remote_list_boxes.borrow().iter() {
            let matched = refilter(list_box);
            any_remote_match |= matched;
            if let Some(expander) = list_box
                .ancestor(gtk::Expander::static_type())
                .and_downcast::<gtk::Expander>()
            {
                expander.set_visible(matched || !filtering);
                if filtering {
                    expander.set_expanded(true);
                }
            }
        }
        any_match |= any_remote_match;
        self.remotes_expander
            .set_visible(any_remote_match || !filtering);
        self.remotes_expander
            .set_expanded(filtering || self.settings.boolean("remotes-expanded"));

        self.no_matches_label.set_visible(filtering && !any_match);
    }

    /// Switch to the first ref (in display order) matching the filter.
    fn activate_first_match(&self) {
        // The entry debounces `search-changed`, so apply the latest text first.
        self.apply_filter();
        let query = self.filter_query.borrow().clone();
        if query.is_empty() {
            return;
        }

        let first_match = [
            &self.branches_list_box,
            &self.tags_list_box,
            &self.stashes_list_box,
        ]
        .into_iter()
        .chain(self.remote_list_boxes.borrow().iter())
        .find_map(|list_box| list_box_rows(list_box).find(|row| row_matches_filter(row, &query)));

        if let Some(row) = first_match {
            row.activate();
        }
    }

    /// Select a ref by name.
    ///
    /// # Arguments
    /// * `ref_name` - Name of the branch, remote branch, tag, or stash to select
    ///
    /// # Returns
    /// `true` if the ref was found and selected, `false` otherwise.
    pub fn select_ref(&self, ref_name: &str) -> bool {
        let remote_list_boxes = self.remote_list_boxes.borrow();
        let found = [&self.branches_list_box]
            .into_iter()
            .chain(remote_list_boxes.iter())
            .chain([&self.tags_list_box, &self.stashes_list_box])
            .find_map(|list_box| {
                find_row_by_ref_name(list_box, ref_name).map(|row| (list_box, row))
            });

        let Some((list_box, (row, ref_info))) = found else {
            return false;
        };
        list_box.select_row(Some(&row));
        *self.selected_ref.borrow_mut() = Some(ref_info);
        true
    }

    /// Register a callback for when a ref is selected (activated).
    ///
    /// # Arguments
    /// * `callback` - Function called with the ref name and type when activated
    pub fn on_ref_selected<F: Fn(&str, RefType) + Clone + 'static>(&self, callback: F) {
        let handler: Rc<dyn Fn(&str, RefType)> = Rc::new(move |name, ty| callback(name, ty));
        *self.activate_handler.borrow_mut() = Some(handler.clone());

        let selected_ref = self.selected_ref.clone();
        let handler_for_branches = handler.clone();
        self.branches_list_box.connect_row_activated(move |_, row| {
            if let Some(ref_info) = row_ref_info(row) {
                *selected_ref.borrow_mut() = Some(ref_info.clone());
                handler_for_branches(&ref_info.name, ref_info.ref_type);
            }
        });

        let selected_ref = self.selected_ref.clone();
        let handler_for_tags = handler.clone();
        self.tags_list_box.connect_row_activated(move |_, row| {
            if let Some(ref_info) = row_ref_info(row) {
                *selected_ref.borrow_mut() = Some(ref_info.clone());
                handler_for_tags(&ref_info.name, ref_info.ref_type);
            }
        });

        let selected_ref = self.selected_ref.clone();
        let handler_for_stashes = handler;
        self.stashes_list_box.connect_row_activated(move |_, row| {
            if let Some(ref_info) = row_ref_info(row) {
                *selected_ref.borrow_mut() = Some(ref_info.clone());
                handler_for_stashes(&ref_info.name, ref_info.ref_type);
            }
        });

        // Remote list boxes are wired when populated in `populate_remotes_section`.
    }

    /// Register a callback for create and delete requests from the header
    /// buttons and row context menus.
    pub fn on_ref_action<F: Fn(RefAction) + 'static>(&self, callback: F) {
        *self.ref_action_handler.borrow_mut() = Some(Rc::new(callback));
    }

    fn selected_row_ref_info(&self) -> Option<SelectedRef> {
        self.branches_list_box
            .selected_row()
            .and_then(|r| row_ref_info(&r))
            .or_else(|| {
                self.tags_list_box
                    .selected_row()
                    .and_then(|r| row_ref_info(&r))
            })
            .or_else(|| {
                self.stashes_list_box
                    .selected_row()
                    .and_then(|r| row_ref_info(&r))
            })
            .or_else(|| {
                self.remote_list_boxes
                    .borrow()
                    .iter()
                    .find_map(|lb| lb.selected_row().and_then(|r| row_ref_info(&r)))
            })
    }

    fn any_row_selected(&self) -> bool {
        self.branches_list_box.selected_row().is_some()
            || self.tags_list_box.selected_row().is_some()
            || self.stashes_list_box.selected_row().is_some()
            || self
                .remote_list_boxes
                .borrow()
                .iter()
                .any(|lb| lb.selected_row().is_some())
    }

    /// Ensure a default ref is selected if nothing is currently selected.
    fn ensure_default_selection(&self) {
        if self.any_row_selected() {
            return;
        }

        // Prefer main if present.
        if self.select_ref("main") {
            return;
        }

        // Otherwise select first selectable row in branches if present.
        if let Some(row) = self.branches_list_box.row_at_index(0) {
            if row.is_selectable() {
                self.branches_list_box.select_row(Some(&row));
                if let Some(ref_info) = row_ref_info(&row) {
                    *self.selected_ref.borrow_mut() = Some(ref_info);
                }
                return;
            }
        }

        // Otherwise select first selectable row in tags if present.
        if let Some(row) = self.tags_list_box.row_at_index(0) {
            if row.is_selectable() {
                self.tags_list_box.select_row(Some(&row));
                if let Some(ref_info) = row_ref_info(&row) {
                    *self.selected_ref.borrow_mut() = Some(ref_info);
                }
            }
        }
    }
}

// =============================================================================
// Private helper functions
// =============================================================================

/// Populate the branches list box.
fn populate_branches_list(
    list_box: &gtk::ListBox,
    branches: &[BranchInfo],
    checked_out_branch: Option<&str>,
) {
    let sorted_branches = sort_branches(branches);
    for branch_info in &sorted_branches {
        let row = create_branch_row(branch_info, checked_out_branch);
        list_box.append(&row);
    }
}

/// Populate the remotes section with one nested expander per remote name.
fn populate_remotes_section(
    content_box: &gtk::Box,
    remote_list_boxes: &Rc<RefCell<Vec<gtk::ListBox>>>,
    local_list_boxes: &[&gtk::ListBox],
    selected_ref: &Rc<RefCell<Option<SelectedRef>>>,
    activate_handler: &Rc<RefCell<Option<Rc<dyn Fn(&str, RefType)>>>>,
    filter_query: &Rc<RefCell<String>>,
    remote_branches: &[BranchInfo],
) {
    let grouped = group_remote_branches(remote_branches);

    for (remote_name, branches) in grouped {
        let remote_expander = gtk::Expander::builder().expanded(true).build();
        remote_expander.add_css_class("branch-panel-expander");
        remote_expander.add_css_class("branch-panel-remote-expander");

        let remote_label = gtk::Label::builder()
            .label(&remote_name)
            .halign(gtk::Align::Start)
            .build();
        remote_label.add_css_class("heading");
        let remote_label_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .build();
        remote_label_box.add_css_class("branch-panel-expander-label");
        remote_label_box.append(&remote_label);
        set_label_count_tooltip(&remote_label, branches.len(), "branch", "branches");
        remote_expander.set_label_widget(Some(&remote_label_box));
        set_expander_chevron_margin(&remote_expander, 10);

        let list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();

        for branch_info in &branches {
            list_box.append(&create_remote_branch_row(branch_info));
        }
        set_ref_filter_func(&list_box, filter_query);

        wire_remote_list_box(
            &list_box,
            local_list_boxes,
            remote_list_boxes,
            selected_ref,
            activate_handler,
        );

        remote_expander.set_child(Some(&list_box));
        content_box.append(&remote_expander);
        remote_list_boxes.borrow_mut().push(list_box);
    }
}

fn clear_remotes_section(
    content_box: &gtk::Box,
    remote_list_boxes: &Rc<RefCell<Vec<gtk::ListBox>>>,
) {
    while let Some(child) = content_box.first_child() {
        content_box.remove(&child);
    }
    remote_list_boxes.borrow_mut().clear();
}

/// Group remote-tracking branches by remote name (the part before the first `/`).
fn group_remote_branches(remote_branches: &[BranchInfo]) -> Vec<(String, Vec<BranchInfo>)> {
    let mut by_remote: BTreeMap<String, Vec<BranchInfo>> = BTreeMap::new();

    for branch in remote_branches {
        let Some((remote, _)) = branch.name.split_once('/') else {
            continue;
        };
        by_remote
            .entry(remote.to_string())
            .or_default()
            .push(branch.clone());
    }

    let mut groups: Vec<(String, Vec<BranchInfo>)> = by_remote.into_iter().collect();
    for (_, branches) in &mut groups {
        branches.sort_by(|a, b| b.latest_commit_time.cmp(&a.latest_commit_time));
    }
    groups
}

fn remote_branch_short_name(full_name: &str) -> &str {
    full_name
        .split_once('/')
        .map(|(_, b)| b)
        .unwrap_or(full_name)
}

/// Keep selection exclusive across the branch, tag, stash, and remote lists.
fn wire_local_list_selection(
    local_list_boxes: &[&gtk::ListBox],
    remote_list_boxes: &Rc<RefCell<Vec<gtk::ListBox>>>,
    selected_ref: &Rc<RefCell<Option<SelectedRef>>>,
) {
    for &list_box in local_list_boxes {
        let others: Vec<gtk::ListBox> = local_list_boxes
            .iter()
            .filter(|&&lb| lb != list_box)
            .map(|&lb| lb.clone())
            .collect();
        let remotes = remote_list_boxes.clone();
        let selected = selected_ref.clone();
        list_box.connect_row_selected(move |_, row| {
            if let Some(r) = row {
                for lb in &others {
                    lb.unselect_all();
                }
                unselect_remote_lists(&remotes, None);
                if let Some(ref_info) = row_ref_info(r) {
                    *selected.borrow_mut() = Some(ref_info);
                }
            }
        });
    }
}

fn wire_remote_list_box(
    list_box: &gtk::ListBox,
    local_list_boxes: &[&gtk::ListBox],
    remote_list_boxes: &Rc<RefCell<Vec<gtk::ListBox>>>,
    selected_ref: &Rc<RefCell<Option<SelectedRef>>>,
    activate_handler: &Rc<RefCell<Option<Rc<dyn Fn(&str, RefType)>>>>,
) {
    let local_lbs: Vec<gtk::ListBox> = local_list_boxes.iter().map(|&lb| lb.clone()).collect();
    let remotes_rc = remote_list_boxes.clone();
    let this_lb = list_box.clone();
    let selected = selected_ref.clone();
    list_box.connect_row_selected(move |_, row| {
        if row.is_some() {
            for lb in &local_lbs {
                lb.unselect_all();
            }
            unselect_remote_lists(&remotes_rc, Some(&this_lb));
            if let Some(r) = row {
                if let Some(ref_info) = row_ref_info(r) {
                    *selected.borrow_mut() = Some(ref_info);
                }
            }
        }
    });

    let selected = selected_ref.clone();
    let handler_rc = activate_handler.clone();
    list_box.connect_row_activated(move |_, row| {
        if let Some(ref_info) = row_ref_info(row) {
            *selected.borrow_mut() = Some(ref_info.clone());
            if let Some(handler) = handler_rc.borrow().as_ref() {
                handler(&ref_info.name, ref_info.ref_type);
            }
        }
    });
}

fn unselect_remote_lists(
    remote_list_boxes: &Rc<RefCell<Vec<gtk::ListBox>>>,
    except: Option<&gtk::ListBox>,
) {
    for lb in remote_list_boxes.borrow().iter() {
        if except.is_none_or(|skip| lb != skip) {
            lb.unselect_all();
        }
    }
}

/// Find the row in `list_box` whose ref is named `ref_name`.
fn find_row_by_ref_name(
    list_box: &gtk::ListBox,
    ref_name: &str,
) -> Option<(gtk::ListBoxRow, SelectedRef)> {
    let mut i = 0;
    while let Some(row) = list_box.row_at_index(i) {
        if let Some(ref_info) = row_ref_info(&row).filter(|info| info.name == ref_name) {
            return Some((row, ref_info));
        }
        i += 1;
    }
    None
}

/// The rows of `list_box`, in display order.
fn list_box_rows(list_box: &gtk::ListBox) -> impl Iterator<Item = gtk::ListBoxRow> + '_ {
    (0..).map_while(|i| list_box.row_at_index(i))
}

/// Hide the rows of `list_box` that don't match the shared filter text.
fn set_ref_filter_func(list_box: &gtk::ListBox, filter_query: &Rc<RefCell<String>>) {
    let filter_query = filter_query.clone();
    list_box.set_filter_func(move |row| row_matches_filter(row, &filter_query.borrow()));
}

/// Whether `row` should be listed for the (already lowercased) filter `query`.
fn row_matches_filter(row: &gtk::ListBoxRow, query: &str) -> bool {
    query.is_empty() || row_filter_text(row).is_some_and(|text| filter_matches(&text, query))
}

/// The text a row is filtered on: its ref name, plus the message for a stash
/// (which is what the row displays; the name is only `stash@{N}`).
fn row_filter_text(row: &gtk::ListBoxRow) -> Option<String> {
    let ref_info = row_ref_info(row)?;
    if ref_info.ref_type != RefType::Stash {
        return Some(ref_info.name);
    }

    let mut current = row.child()?.first_child();
    while let Some(widget) = current {
        if widget.widget_name() == "stash-message" {
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                return Some(format!("{} {}", ref_info.name, label.text()));
            }
        }
        current = widget.next_sibling();
    }
    Some(ref_info.name)
}

/// Case-insensitive substring match. `query` must already be lowercased.
fn filter_matches(text: &str, query: &str) -> bool {
    text.to_lowercase().contains(query)
}

/// Populate the stashes list box, hiding the section when there are none.
fn populate_stashes_list(expander: &gtk::Expander, list_box: &gtk::ListBox, stashes: &[StashInfo]) {
    for stash_info in stashes {
        list_box.append(&create_stash_row(stash_info));
    }
    expander.set_visible(!stashes.is_empty());
}

/// Populate the tags list box.
fn populate_tags_list(list_box: &gtk::ListBox, tags: &[TagInfo]) {
    let sorted_tags = sort_tags(tags);
    for tag_info in &sorted_tags {
        let row = create_tag_row(tag_info);
        list_box.append(&row);
    }
}

/// Format a hover tooltip describing how many refs are in a section.
fn format_count_tooltip(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// Show branch, tag, stash, and remote totals on the section labels.
#[allow(clippy::too_many_arguments)]
fn set_section_count_tooltips(
    branches_label: &gtk::Label,
    tags_label: &gtk::Label,
    stashes_label: &gtk::Label,
    remotes_label: &gtk::Label,
    branches: &[BranchInfo],
    remote_branches: &[BranchInfo],
    tags: &[TagInfo],
    stashes: &[StashInfo],
) {
    set_label_count_tooltip(branches_label, branches.len(), "branch", "branches");
    set_label_count_tooltip(tags_label, tags.len(), "tag", "tags");
    set_label_count_tooltip(stashes_label, stashes.len(), "stash", "stashes");
    set_label_count_tooltip(
        remotes_label,
        group_remote_branches(remote_branches).len(),
        "remote",
        "remotes",
    );
}

fn set_label_count_tooltip(label: &gtk::Label, count: usize, singular: &str, plural: &str) {
    let text = format_count_tooltip(count, singular, plural);
    label.set_tooltip_text(Some(&text));
    if let Some(parent) = label.parent() {
        parent.set_tooltip_text(Some(&text));
    }
}

/// Add margin to the expander chevron icon.
fn set_expander_chevron_margin(expander: &gtk::Expander, margin_start: i32) {
    fn apply_to_builtin_icon(widget: &gtk::Widget, margin_start: i32) -> bool {
        if widget.type_().name() == "GtkBuiltinIcon" {
            widget.set_margin_start(margin_start);
            return true;
        }

        let mut child = widget.first_child();
        while let Some(current) = child {
            if apply_to_builtin_icon(&current, margin_start) {
                return true;
            }
            child = current.next_sibling();
        }

        false
    }

    let _ = apply_to_builtin_icon(expander.upcast_ref(), margin_start);
}

/// Add a small "+" button to the end of a section header that activates `action`.
fn append_header_button(label_box: &gtk::Box, tooltip: &str, action: &str) {
    label_box.set_hexpand(true);
    let button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text(tooltip)
        .action_name(action)
        .halign(gtk::Align::End)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "branch-panel-header-button"])
        .build();
    label_box.append(&button);
}

/// Install the `refs.*` actions on the panel. `refs.delete` acts on
/// `context_ref`, the row that was last right-clicked. Returns the delete
/// action so the context menu can disable it for the checked-out branch.
fn install_ref_actions(
    panel: &gtk::Box,
    handler: &Rc<RefCell<Option<Rc<dyn Fn(RefAction)>>>>,
    context_ref: &Rc<RefCell<Option<SelectedRef>>>,
) -> gio::SimpleAction {
    let emit = {
        let handler = handler.clone();
        move |action: RefAction| {
            // Clone out of the cell so the handler may re-register itself.
            let handler = handler.borrow().clone();
            if let Some(handler) = handler {
                handler(action);
            }
        }
    };

    let action_group = gio::SimpleActionGroup::new();

    let new_branch = gio::SimpleAction::new("new-branch", None);
    let emit_new_branch = emit.clone();
    new_branch.connect_activate(move |_, _| emit_new_branch(RefAction::NewBranch));
    action_group.add_action(&new_branch);

    let new_tag = gio::SimpleAction::new("new-tag", None);
    let emit_new_tag = emit.clone();
    new_tag.connect_activate(move |_, _| emit_new_tag(RefAction::NewTag));
    action_group.add_action(&new_tag);

    let delete = gio::SimpleAction::new("delete", None);
    let context_ref = context_ref.clone();
    delete.connect_activate(move |_, _| {
        let target = context_ref.borrow_mut().take();
        if let Some(SelectedRef { name, ref_type }) = target {
            emit(RefAction::Delete { name, ref_type });
        }
    });
    action_group.add_action(&delete);

    panel.insert_action_group("refs", Some(&action_group));
    delete
}

/// Show `menu` in `popover` when a row of `list_box` is right-clicked. The
/// list box is kept across refreshes while its rows are rebuilt, so the
/// gesture is attached here and the clicked row is looked up at click time.
fn attach_context_menu(
    list_box: &gtk::ListBox,
    menu: &gio::Menu,
    popover: &gtk::PopoverMenu,
    context_ref: &Rc<RefCell<Option<SelectedRef>>>,
    checked_out_branch: &Rc<RefCell<Option<String>>>,
    delete_action: &gio::SimpleAction,
) {
    let right_click = gtk::GestureClick::new();
    right_click.set_button(3);
    let list_box_for_click = list_box.clone();
    let context_ref = context_ref.clone();
    let checked_out_branch = checked_out_branch.clone();
    let delete_action = delete_action.clone();
    let popover = popover.clone();
    let menu = menu.clone();
    right_click.connect_pressed(move |gesture, _, x, y| {
        let Some(ref_info) = list_box_for_click
            .row_at_y(y as i32)
            .and_then(|row| row_ref_info(&row))
        else {
            return;
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);

        let is_checked_out = ref_info.ref_type == RefType::Branch
            && checked_out_branch.borrow().as_deref() == Some(ref_info.name.as_str());
        delete_action.set_enabled(!is_checked_out);
        *context_ref.borrow_mut() = Some(ref_info);

        // The popover lives on the panel root, so point at the click there.
        let Some(parent) = popover.parent() else {
            return;
        };
        let Some(point) = list_box_for_click
            .compute_point(&parent, &gtk::graphene::Point::new(x as f32, y as f32))
        else {
            return;
        };
        popover.set_menu_model(Some(&menu));
        popover.set_pointing_to(Some(&gtk::gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        popover.popup();
    });
    list_box.add_controller(right_click);
}

/// Extract the ref info (name and type) from a list box row.
fn row_ref_info(row: &gtk::ListBoxRow) -> Option<SelectedRef> {
    let row_name = row.widget_name();
    if let Some(full) = row_name.as_str().strip_prefix("remote-ref:") {
        return Some(SelectedRef {
            name: full.to_string(),
            ref_type: RefType::Remote,
        });
    }
    if let Some(name) = row_name.as_str().strip_prefix("stash-ref:") {
        return Some(SelectedRef {
            name: name.to_string(),
            ref_type: RefType::Stash,
        });
    }

    let child = row.child()?;
    let box_widget = child.downcast_ref::<gtk::Box>()?;

    let mut name = None;
    let mut ref_type = None;

    let mut current = box_widget.first_child();
    while let Some(widget) = current {
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            match label.widget_name().as_str() {
                "branch-name" => {
                    name = Some(label.text().to_string());
                    ref_type = Some(RefType::Branch);
                }
                "tag-name" => {
                    name = Some(label.text().to_string());
                    ref_type = Some(RefType::Tag);
                }
                _ => {}
            }
        }
        current = widget.next_sibling();
    }

    match (name, ref_type) {
        (Some(name), Some(ref_type)) => Some(SelectedRef { name, ref_type }),
        _ => None,
    }
}

/// Sort branches with "main" or "master" first, then by latest commit time.
fn sort_branches(branches: &[BranchInfo]) -> Vec<BranchInfo> {
    let mut sorted = branches.to_vec();
    sorted.sort_by(|a, b| {
        branch_priority(&a.name)
            .cmp(&branch_priority(&b.name))
            .then_with(|| {
                b.latest_commit_time
                    .cmp(&a.latest_commit_time)
                    .then_with(|| natural_compare(&a.name, &b.name))
            })
    });
    sorted
}

fn branch_priority(name: &str) -> u8 {
    match name {
        "main" => 0,
        "master" => 1,
        _ => 2,
    }
}

/// Sort tags alphabetically using natural sort (reverse order so newest versions appear first).
pub(crate) fn sort_tags(tags: &[TagInfo]) -> Vec<TagInfo> {
    let mut sorted = tags.to_vec();
    sorted.sort_by(|a, b| natural_compare(&b.name, &a.name));
    sorted
}

/// Create a GTK row widget for a branch.
fn create_branch_row(
    branch_info: &BranchInfo,
    checked_out_branch: Option<&str>,
) -> gtk::ListBoxRow {
    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .spacing(8)
        .build();

    let is_checked_out = checked_out_branch.is_some_and(|b| b == branch_info.name);
    let check_icon = gtk::Image::from_icon_name("object-select-symbolic");
    check_icon.set_pixel_size(16);
    check_icon.set_opacity(if is_checked_out { 1.0 } else { 0.0 });
    check_icon.set_tooltip_text(if is_checked_out {
        Some("Checked out branch")
    } else {
        None
    });
    row_box.append(&check_icon);

    let branch_label = gtk::Label::builder().halign(gtk::Align::Start).build();
    branch_label.set_widget_name("branch-name");
    branch_label.set_text(&branch_info.name);
    branch_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    branch_label.set_tooltip_text(Some(&branch_info.name));
    row_box.append(&branch_label);

    let time_label = gtk::Label::builder()
        .halign(gtk::Align::End)
        .hexpand(true)
        .build();

    let time_ago = format_time_ago(branch_info.latest_commit_time);
    let markup = format!(
        "<span size='small'>{}</span>",
        gtk::glib::markup_escape_text(&time_ago)
    );
    time_label.set_markup(&markup);
    time_label.add_css_class("dim-label");

    row_box.append(&time_label);
    row_box.set_hexpand(true);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row
}

/// Create a GTK row widget for a remote-tracking branch.
fn create_remote_branch_row(branch_info: &BranchInfo) -> gtk::ListBoxRow {
    let short_name = remote_branch_short_name(&branch_info.name);

    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .spacing(8)
        .build();

    let check_icon = gtk::Image::from_icon_name("object-select-symbolic");
    check_icon.set_pixel_size(16);
    check_icon.set_opacity(0.0);
    row_box.append(&check_icon);

    let branch_label = gtk::Label::builder().halign(gtk::Align::Start).build();
    branch_label.set_text(short_name);
    branch_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    branch_label.set_tooltip_text(Some(&branch_info.name));
    row_box.append(&branch_label);

    let time_label = gtk::Label::builder()
        .halign(gtk::Align::End)
        .hexpand(true)
        .build();

    let time_ago = format_time_ago(branch_info.latest_commit_time);
    let markup = format!(
        "<span size='small'>{}</span>",
        gtk::glib::markup_escape_text(&time_ago)
    );
    time_label.set_markup(&markup);
    time_label.add_css_class("dim-label");

    row_box.append(&time_label);
    row_box.set_hexpand(true);

    let row = gtk::ListBoxRow::new();
    row.set_widget_name(&format!("remote-ref:{}", branch_info.name));
    row.set_child(Some(&row_box));
    row
}

/// Create a GTK row widget for a tag.
fn create_tag_row(tag_info: &TagInfo) -> gtk::ListBoxRow {
    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .spacing(8)
        .build();

    // Placeholder for checkmark (keeps alignment consistent with branches, but tags can't be "checked out")
    let check_icon = gtk::Image::from_icon_name("object-select-symbolic");
    check_icon.set_pixel_size(16);
    check_icon.set_opacity(0.0);
    row_box.append(&check_icon);

    // Tag name label with ellipsis for long names
    let tag_label = gtk::Label::builder().halign(gtk::Align::Start).build();
    tag_label.set_widget_name("tag-name");
    tag_label.set_text(&tag_info.name);
    tag_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    tag_label.set_tooltip_text(Some(&tag_info.name));
    row_box.append(&tag_label);

    // Time ago label (lighter color, smaller font, right-aligned)
    let time_label = gtk::Label::builder()
        .halign(gtk::Align::End)
        .hexpand(true)
        .build();

    let time_ago = format_time_ago(tag_info.commit_time);
    let markup = format!(
        "<span size='small'>{}</span>",
        gtk::glib::markup_escape_text(&time_ago)
    );
    time_label.set_markup(&markup);
    time_label.add_css_class("dim-label");

    row_box.append(&time_label);
    row_box.set_hexpand(true);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&row_box));
    row
}

/// Create a GTK row widget for a stash entry.
fn create_stash_row(stash_info: &StashInfo) -> gtk::ListBoxRow {
    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .spacing(8)
        .build();

    // Placeholder for checkmark (keeps alignment consistent with branches)
    let check_icon = gtk::Image::from_icon_name("object-select-symbolic");
    check_icon.set_pixel_size(16);
    check_icon.set_opacity(0.0);
    row_box.append(&check_icon);

    let stash_label = gtk::Label::builder().halign(gtk::Align::Start).build();
    stash_label.set_widget_name("stash-message");
    stash_label.set_text(&stash_info.message);
    stash_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    stash_label.set_tooltip_text(Some(&format!(
        "{}: {}",
        stash_info.name, stash_info.message
    )));
    row_box.append(&stash_label);

    let time_label = gtk::Label::builder()
        .halign(gtk::Align::End)
        .hexpand(true)
        .build();

    let time_ago = format_time_ago(stash_info.time);
    let markup = format!(
        "<span size='small'>{}</span>",
        gtk::glib::markup_escape_text(&time_ago)
    );
    time_label.set_markup(&markup);
    time_label.add_css_class("dim-label");

    row_box.append(&time_label);
    row_box.set_hexpand(true);

    let row = gtk::ListBoxRow::new();
    row.set_widget_name(&format!("stash-ref:{}", stash_info.name));
    row.set_child(Some(&row_box));
    row
}

/// Natural comparison for strings (e.g., "branch-2" < "branch-10").
fn natural_compare(a: &str, b: &str) -> std::cmp::Ordering {
    let mut a_chars = a.chars().peekable();
    let mut b_chars = b.chars().peekable();

    loop {
        let a_done = a_chars.peek().is_none();
        let b_done = b_chars.peek().is_none();

        if a_done && b_done {
            return std::cmp::Ordering::Equal;
        }
        if a_done {
            return std::cmp::Ordering::Less;
        }
        if b_done {
            return std::cmp::Ordering::Greater;
        }

        let a_is_digit = a_chars.peek().map(|c| c.is_ascii_digit()).unwrap_or(false);
        let b_is_digit = b_chars.peek().map(|c| c.is_ascii_digit()).unwrap_or(false);

        if a_is_digit && b_is_digit {
            // Both start with digits - compare numerically
            let a_num: String = a_chars
                .by_ref()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let b_num: String = b_chars
                .by_ref()
                .take_while(|c| c.is_ascii_digit())
                .collect();

            let a_val: u64 = a_num.parse().unwrap_or(0);
            let b_val: u64 = b_num.parse().unwrap_or(0);

            match a_val.cmp(&b_val) {
                std::cmp::Ordering::Equal => continue,
                other => return other,
            }
        } else {
            // At least one is not a digit - compare lexicographically
            let a_char = a_chars.next().unwrap();
            let b_char = b_chars.next().unwrap();

            match a_char
                .to_ascii_lowercase()
                .cmp(&b_char.to_ascii_lowercase())
            {
                std::cmp::Ordering::Equal => {
                    // If case-insensitive equal, compare case-sensitive
                    match a_char.cmp(&b_char) {
                        std::cmp::Ordering::Equal => continue,
                        other => return other,
                    }
                }
                other => return other,
            }
        }
    }
}

/// Format a datetime as a relative time string (e.g., "2h", "3d", "1mo").
fn format_time_ago(dt: DateTime<Utc>) -> String {
    let now = Utc::now();
    let duration = now.signed_duration_since(dt);

    let total_seconds = duration.num_seconds();
    let total_minutes = duration.num_minutes();
    let total_hours = duration.num_hours();
    let total_days = duration.num_days();
    let total_weeks = total_days / 7;
    let total_months = total_days / 30;
    let total_years = total_days / 365;

    if total_seconds < 60 {
        "just now".to_string()
    } else if total_minutes < 60 {
        format!("{}m", total_minutes)
    } else if total_hours < 24 {
        format!("{}h", total_hours)
    } else if total_days < 7 {
        format!("{}d", total_days)
    } else if total_months == 0 {
        // Display weeks for < 1 month (includes 28-29 days which would show "0m")
        format!("{}w", total_weeks)
    } else if total_months < 12 {
        format!("{}mo", total_months)
    } else {
        format!("{}y", total_years)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn branch(name: &str, timestamp: i64) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            latest_commit_time: Utc.timestamp_opt(timestamp, 0).unwrap(),
        }
    }

    #[test]
    fn sort_branches_orders_newest_first() {
        let sorted = sort_branches(&[
            branch("oldest", 100),
            branch("newest", 300),
            branch("middle", 200),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["newest", "middle", "oldest"]);
    }

    #[test]
    fn sort_branches_pins_main_above_newer_branches() {
        let sorted = sort_branches(&[
            branch("main", 100),
            branch("feature", 300),
            branch("bugfix", 200),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["main", "feature", "bugfix"]);
    }

    #[test]
    fn sort_branches_pins_master_above_newer_branches() {
        let sorted = sort_branches(&[
            branch("feature", 300),
            branch("master", 100),
            branch("bugfix", 200),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["master", "feature", "bugfix"]);
    }

    #[test]
    fn sort_branches_orders_main_before_master() {
        let sorted = sort_branches(&[
            branch("master", 300),
            branch("main", 100),
            branch("feature", 200),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["main", "master", "feature"]);
    }

    #[test]
    fn sort_branches_uses_natural_name_order_for_ties() {
        let sorted = sort_branches(&[
            branch("branch-10", 100),
            branch("branch-2", 100),
            branch("branch-1", 100),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["branch-1", "branch-2", "branch-10"]);
    }

    #[test]
    fn filter_matches_ignores_case_of_the_ref_name() {
        assert!(filter_matches("Feature/Login-Page", "login"));
        assert!(filter_matches("v2026.6", "v2026"));
        assert!(filter_matches(
            "stash@{0} WIP on main: fix crash",
            "on main"
        ));
    }

    #[test]
    fn filter_matches_rejects_refs_without_the_query() {
        assert!(!filter_matches("feature/login", "logout"));
        assert!(!filter_matches("main", "maintenance"));
    }

    #[test]
    fn format_count_tooltip_uses_singular_for_one() {
        assert_eq!(format_count_tooltip(1, "branch", "branches"), "1 branch");
        assert_eq!(format_count_tooltip(1, "tag", "tags"), "1 tag");
        assert_eq!(format_count_tooltip(1, "remote", "remotes"), "1 remote");
    }

    #[test]
    fn format_count_tooltip_uses_plural_for_zero_and_many() {
        assert_eq!(format_count_tooltip(0, "branch", "branches"), "0 branches");
        assert_eq!(format_count_tooltip(12, "tag", "tags"), "12 tags");
        assert_eq!(format_count_tooltip(2, "remote", "remotes"), "2 remotes");
    }
}
