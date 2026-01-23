//! Branch panel UI component for displaying and selecting git branches and tags.
//!
//! This module provides the `BranchPanel` widget which displays a list of
//! git branches and tags with their last commit time and allows single-selection.
//! Both sections are collapsible with state persisted to gsettings.

use chrono::{DateTime, Utc};
use gtk::{gio, prelude::*};
use std::cell::RefCell;
use std::rc::Rc;

use crate::APP_ID;
use crate::git::{BranchInfo, TagInfo};

/// Type of git reference (branch or tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefType {
    Branch,
    Tag,
}

/// Information about the currently selected reference.
#[derive(Clone)]
struct SelectedRef {
    name: String,
    ref_type: RefType,
}

/// A panel widget that displays a list of git branches and tags.
///
/// The panel shows branches and tags in separate collapsible sections, each with:
/// - A collapsible section header ("Branches" / "Tags")
/// - A checkmark icon for the currently checked-out branch or viewed tag
/// - The ref name (with ellipsis for long names)
/// - Relative time since last commit
///
/// Branches are sorted with "main" first, then alphabetically using
/// natural sort order (e.g., "branch-2" comes before "branch-10").
/// Tags are sorted alphabetically using natural sort order.
///
/// Expanded/collapsed state is persisted to gsettings.
#[derive(Clone)]
pub struct BranchPanel {
    /// The root widget container
    pub widget: gtk::Box,
    /// The list box containing branch rows
    branches_list_box: gtk::ListBox,
    /// The list box containing tag rows
    tags_list_box: gtk::ListBox,
    /// The expander for branches section (kept for widget lifetime)
    _branches_expander: gtk::Expander,
    /// The expander for tags section (kept for widget lifetime)
    _tags_expander: gtk::Expander,
    /// Currently selected reference (name and type)
    selected_ref: Rc<RefCell<Option<SelectedRef>>>,
    /// GSettings for persisting expanded state (kept for reference lifetime)
    _settings: gio::Settings,
}

impl BranchPanel {
    /// Create a new BranchPanel with the given branches.
    ///
    /// # Arguments
    /// * `branches` - Slice of branch information to display
    pub fn new(branches: &[BranchInfo]) -> Self {
        Self::new_with_refs(branches, &[], None, None)
    }

    /// Create a new BranchPanel with branches, tags, and indication of current state.
    ///
    /// # Arguments
    /// * `branches` - Slice of branch information to display
    /// * `tags` - Slice of tag information to display
    /// * `checked_out_branch` - Name of the currently checked out branch (if any)
    /// * `current_ref_name` - Name of the currently viewed ref (branch or tag)
    pub fn new_with_refs(
        branches: &[BranchInfo],
        tags: &[TagInfo],
        checked_out_branch: Option<&str>,
        current_ref_name: Option<&str>,
    ) -> Self {
        let settings = gio::Settings::new(APP_ID);

        let side_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["branch-panel"])
            .build();
        side_panel.set_width_request(150);

        // Create a scrolled window for the content
        let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();

        // Create container for expanders
        let content_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

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
        branches_expander.set_label_widget(Some(&branches_label_box));
        set_expander_chevron_margin(&branches_expander, 10);

        let branches_list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();

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
        tags_expander.set_label_widget(Some(&tags_label_box));
        set_expander_chevron_margin(&tags_expander, 10);

        let tags_list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();

        tags_expander.set_child(Some(&tags_list_box));
        content_box.append(&tags_expander);

        scrolled.set_child(Some(&content_box));
        side_panel.append(&scrolled);
        side_panel.set_vexpand(true);

        // Populate list boxes
        populate_branches_list(&branches_list_box, branches, checked_out_branch);
        populate_tags_list(&tags_list_box, tags);

        // Wire up settings persistence for expanded state
        let settings_for_branches = settings.clone();
        branches_expander.connect_expanded_notify(move |exp| {
            let _ = settings_for_branches.set_boolean("branches-expanded", exp.is_expanded());
        });

        let settings_for_tags = settings.clone();
        tags_expander.connect_expanded_notify(move |exp| {
            let _ = settings_for_tags.set_boolean("tags-expanded", exp.is_expanded());
        });

        let selected_ref = Rc::new(RefCell::new(None));

        // Wire up selection coordination: selecting in one list deselects in the other
        let tags_list_for_branches = tags_list_box.clone();
        let selected_ref_for_branches = selected_ref.clone();
        branches_list_box.connect_row_selected(move |_, row| {
            if row.is_some() {
                tags_list_for_branches.unselect_all();
                // Update selected_ref when row is selected (not just activated)
                if let Some(r) = row {
                    if let Some(ref_info) = row_ref_info(r) {
                        *selected_ref_for_branches.borrow_mut() = Some(ref_info);
                    }
                }
            }
        });

        let branches_list_for_tags = branches_list_box.clone();
        let selected_ref_for_tags = selected_ref.clone();
        tags_list_box.connect_row_selected(move |_, row| {
            if row.is_some() {
                branches_list_for_tags.unselect_all();
                // Update selected_ref when row is selected (not just activated)
                if let Some(r) = row {
                    if let Some(ref_info) = row_ref_info(r) {
                        *selected_ref_for_tags.borrow_mut() = Some(ref_info);
                    }
                }
            }
        });

        let panel = Self {
            widget: side_panel,
            branches_list_box,
            tags_list_box,
            _branches_expander: branches_expander,
            _tags_expander: tags_expander,
            selected_ref,
            _settings: settings,
        };

        // Default selection: prefer current ref, then checked-out branch, then "main", then first branch.
        if let Some(name) = current_ref_name {
            let _ = panel.select_ref(name);
        } else if let Some(name) = checked_out_branch {
            let _ = panel.select_ref(name);
        }
        panel.ensure_default_selection();
        panel
    }

    /// Update the panel with new lists of branches and tags.
    ///
    /// Attempts to preserve the current selection if possible.
    ///
    /// # Arguments
    /// * `branches` - New slice of branch information
    /// * `tags` - New slice of tag information
    /// * `checked_out_branch` - Name of the currently checked out branch (if any)
    /// * `current_ref_name` - Name of the currently viewed ref (branch or tag)
    pub fn update_refs(
        &self,
        branches: &[BranchInfo],
        tags: &[TagInfo],
        checked_out_branch: Option<&str>,
        current_ref_name: Option<&str>,
    ) {
        // Preserve the last selected ref (or current selected row if present).
        let preserved = self.selected_ref.borrow().clone().or_else(|| {
            self.branches_list_box
                .selected_row()
                .and_then(|r| row_ref_info(&r))
                .or_else(|| {
                    self.tags_list_box
                        .selected_row()
                        .and_then(|r| row_ref_info(&r))
                })
        });

        // Clear existing rows from branches list
        while let Some(row) = self.branches_list_box.row_at_index(0) {
            self.branches_list_box.remove(&row);
        }

        // Clear existing rows from tags list
        while let Some(row) = self.tags_list_box.row_at_index(0) {
            self.tags_list_box.remove(&row);
        }

        // Add new branches and tags
        populate_branches_list(&self.branches_list_box, branches, checked_out_branch);
        populate_tags_list(&self.tags_list_box, tags);

        // Restore selection
        if let Some(ref_info) = preserved {
            let _ = self.select_ref(&ref_info.name);
        } else if let Some(name) = current_ref_name {
            let _ = self.select_ref(name);
        } else if let Some(name) = checked_out_branch {
            let _ = self.select_ref(name);
        }
        self.ensure_default_selection();
    }

    /// Update the panel with a new list of branches (legacy API for compatibility).
    ///
    /// # Arguments
    /// * `branches` - New slice of branch information
    /// * `checked_out_branch` - Name of the currently checked out branch (if any)
    pub fn update_branches(&self, branches: &[BranchInfo], checked_out_branch: Option<&str>) {
        self.update_refs(branches, &[], checked_out_branch, checked_out_branch);
    }

    /// Select a ref by name.
    ///
    /// # Arguments
    /// * `ref_name` - Name of the branch or tag to select
    ///
    /// # Returns
    /// `true` if the ref was found and selected, `false` otherwise.
    pub fn select_ref(&self, ref_name: &str) -> bool {
        // Search in branches list
        let mut i = 0;
        while let Some(row) = self.branches_list_box.row_at_index(i) {
            if let Some(ref_info) = row_ref_info(&row) {
                if ref_info.name == ref_name {
                    self.branches_list_box.select_row(Some(&row));
                    *self.selected_ref.borrow_mut() = Some(ref_info);
                    return true;
                }
            }
            i += 1;
        }

        // Search in tags list
        let mut i = 0;
        while let Some(row) = self.tags_list_box.row_at_index(i) {
            if let Some(ref_info) = row_ref_info(&row) {
                if ref_info.name == ref_name {
                    self.tags_list_box.select_row(Some(&row));
                    *self.selected_ref.borrow_mut() = Some(ref_info);
                    return true;
                }
            }
            i += 1;
        }

        false
    }

    /// Register a callback for when a ref is selected (activated).
    ///
    /// # Arguments
    /// * `callback` - Function called with the ref name and type when activated
    pub fn on_ref_selected<F: Fn(&str, RefType) + Clone + 'static>(&self, callback: F) {
        let selected_ref = self.selected_ref.clone();
        let callback_clone = callback.clone();
        self.branches_list_box.connect_row_activated(move |_, row| {
            if let Some(ref_info) = row_ref_info(row) {
                *selected_ref.borrow_mut() = Some(ref_info.clone());
                callback_clone(&ref_info.name, ref_info.ref_type);
            }
        });

        let selected_ref = self.selected_ref.clone();
        self.tags_list_box.connect_row_activated(move |_, row| {
            if let Some(ref_info) = row_ref_info(row) {
                *selected_ref.borrow_mut() = Some(ref_info.clone());
                callback(&ref_info.name, ref_info.ref_type);
            }
        });
    }

    /// Ensure a default ref is selected if nothing is currently selected.
    fn ensure_default_selection(&self) {
        if self.branches_list_box.selected_row().is_some()
            || self.tags_list_box.selected_row().is_some()
        {
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

/// Populate the tags list box.
fn populate_tags_list(list_box: &gtk::ListBox, tags: &[TagInfo]) {
    let sorted_tags = sort_tags(tags);
    for tag_info in &sorted_tags {
        let row = create_tag_row(tag_info);
        list_box.append(&row);
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

/// Extract the ref info (name and type) from a list box row.
fn row_ref_info(row: &gtk::ListBoxRow) -> Option<SelectedRef> {
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

/// Sort branches with "main" first, then alphabetically using natural sort.
fn sort_branches(branches: &[BranchInfo]) -> Vec<BranchInfo> {
    let mut sorted = branches.to_vec();
    sorted.sort_by(|a, b| match (a.name == "main", b.name == "main") {
        (true, true) => std::cmp::Ordering::Equal,
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        (false, false) => natural_compare(&a.name, &b.name),
    });
    sorted
}

/// Sort tags alphabetically using natural sort (reverse order so newest versions appear first).
fn sort_tags(tags: &[TagInfo]) -> Vec<TagInfo> {
    let mut sorted = tags.to_vec();
    sorted.sort_by(|a, b| natural_compare(&b.name, &a.name)); // Reverse order
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

    // Checkmark indicator: show only if this is the git-checked-out branch (HEAD)
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

    // Branch name label with ellipsis for long names
    let branch_label = gtk::Label::builder().halign(gtk::Align::Start).build();
    branch_label.set_widget_name("branch-name");
    branch_label.set_text(&branch_info.name);
    branch_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    branch_label.set_tooltip_text(Some(&branch_info.name));
    row_box.append(&branch_label);

    // Time ago label (lighter color, smaller font, right-aligned)
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
