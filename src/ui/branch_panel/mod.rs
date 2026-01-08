//! Branch panel UI component for displaying and selecting git branches and tags.
//!
//! This module provides the `BranchPanel` widget which displays a list of
//! git branches and tags with their last commit time and allows single-selection.

use chrono::{DateTime, Utc};
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::git::{BranchInfo, TagInfo};

/// A panel widget that displays a list of git branches and tags.
///
/// The panel shows branches and tags in separate sections:
/// - **Branches section**: Shows each branch with a checkmark icon for the
///   currently checked-out branch, the branch name, and relative time since last commit.
/// - **Tags section**: Shows each tag with a tag icon and relative time.
///
/// Branches are sorted with "main" first, then alphabetically using
/// natural sort order (e.g., "branch-2" comes before "branch-10").
/// Tags are sorted alphabetically using natural sort order.
#[derive(Clone)]
pub struct BranchPanel {
    /// The root widget container
    pub widget: gtk::Box,
    /// The list box containing branch and tag rows
    list_box: gtk::ListBox,
    /// Currently selected ref name (branch or tag)
    selected_ref: Rc<RefCell<Option<String>>>,
}

impl BranchPanel {
    /// Create a new BranchPanel with the given branches (no tags).
    ///
    /// # Arguments
    /// * `branches` - Slice of branch information to display
    pub fn new(branches: &[BranchInfo]) -> Self {
        Self::new_with_refs(branches, &[], None, None)
    }

    /// Create a new BranchPanel with branches, tags, and indication of which branch is checked out.
    ///
    /// # Arguments
    /// * `branches` - Slice of branch information to display
    /// * `tags` - Slice of tag information to display
    /// * `checked_out_branch` - Name of the currently checked out branch (if any)
    pub fn new_with_refs(
        branches: &[BranchInfo],
        tags: &[TagInfo],
        checked_out_branch: Option<&str>,
        checked_out_tag: Option<&str>,
    ) -> Self {
        let side_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        // Create a scrolled window for the list
        let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();

        // Create a list box to hold branches and tags
        let list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();

        // Make section headers non-selectable
        list_box.set_header_func(|row, _| {
            if row.widget_name() == "section-header" {
                row.set_selectable(false);
                row.set_activatable(false);
            }
        });

        populate_list(
            &list_box,
            branches,
            tags,
            checked_out_branch,
            checked_out_tag,
        );

        scrolled.set_child(Some(&list_box));
        side_panel.append(&scrolled);
        side_panel.set_vexpand(true);

        let panel = Self {
            widget: side_panel,
            list_box,
            selected_ref: Rc::new(RefCell::new(None)),
        };

        // Default selection: prefer checked-out branch, then "main", then first selectable row.
        if let Some(name) = checked_out_branch {
            let _ = panel.select_ref(name);
        }
        panel.ensure_default_selection();
        panel
    }

    /// Update the panel with a new list of branches and tags.
    ///
    /// Attempts to preserve the current selection if possible.
    ///
    /// # Arguments
    /// * `branches` - New slice of branch information
    /// * `tags` - New slice of tag information
    /// * `checked_out_branch` - Name of the currently checked out branch (if any)
    pub fn update_refs(
        &self,
        branches: &[BranchInfo],
        tags: &[TagInfo],
        checked_out_branch: Option<&str>,
        checked_out_tag: Option<&str>,
    ) {
        // Preserve the last selected ref (or current selected row if present).
        let preserved = self
            .selected_ref
            .borrow()
            .clone()
            .or_else(|| self.list_box.selected_row().and_then(|r| row_ref_name(&r)));

        // Clear existing rows
        while let Some(row) = self.list_box.row_at_index(0) {
            self.list_box.remove(&row);
        }

        populate_list(
            &self.list_box,
            branches,
            tags,
            checked_out_branch,
            checked_out_tag,
        );

        if let Some(name) = preserved {
            let _ = self.select_ref(&name);
        } else if let Some(name) = checked_out_branch {
            let _ = self.select_ref(name);
        }
        self.ensure_default_selection();
    }

    /// Select a ref (branch or tag) by name.
    ///
    /// # Arguments
    /// * `ref_name` - Name of the branch or tag to select
    ///
    /// # Returns
    /// `true` if the ref was found and selected, `false` otherwise.
    pub fn select_ref(&self, ref_name: &str) -> bool {
        let mut i = 0;
        while let Some(row) = self.list_box.row_at_index(i) {
            if row.is_selectable() {
                if let Some(name) = row_ref_name(&row) {
                    if name == ref_name {
                        self.list_box.select_row(Some(&row));
                        *self.selected_ref.borrow_mut() = Some(ref_name.to_string());
                        return true;
                    }
                }
            }
            i += 1;
        }
        false
    }

    /// Register a callback for when a ref (branch or tag) is selected (activated).
    ///
    /// # Arguments
    /// * `callback` - Function called with the ref name when a branch or tag is activated
    pub fn ref_selected<F: Fn(&str) + 'static>(&self, callback: F) {
        let selected_ref = self.selected_ref.clone();
        self.list_box.connect_row_activated(move |_, row| {
            if let Some(ref_name) = row_ref_name(row) {
                *selected_ref.borrow_mut() = Some(ref_name.clone());
                callback(&ref_name);
            }
        });
    }

    /// Register a callback for when a branch is selected (legacy method for compatibility).
    pub fn branch_selected<F: Fn(&str) + 'static>(&self, callback: F) {
        self.ref_selected(callback);
    }

    /// Ensure a default ref is selected if nothing is currently selected.
    fn ensure_default_selection(&self) {
        if self.list_box.selected_row().is_some() {
            return;
        }

        // Prefer main if present.
        if self.select_ref("main") {
            return;
        }

        // Otherwise select first selectable row if present.
        let mut i = 0;
        while let Some(row) = self.list_box.row_at_index(i) {
            if row.is_selectable() {
                self.list_box.select_row(Some(&row));
                if let Some(name) = row_ref_name(&row) {
                    *self.selected_ref.borrow_mut() = Some(name);
                }
                return;
            }
            i += 1;
        }
    }
}

// =============================================================================
// Private helper functions
// =============================================================================

/// Populate the list box with branches and tags, including section headers.
fn populate_list(
    list_box: &gtk::ListBox,
    branches: &[BranchInfo],
    tags: &[TagInfo],
    checked_out_branch: Option<&str>,
    checked_out_tag: Option<&str>,
) {
    // Add Branches section
    if !branches.is_empty() {
        let header = create_section_header("Branches");
        list_box.append(&header);

        let sorted_branches = sort_branches(branches);
        for branch_info in &sorted_branches {
            let row = create_branch_row(branch_info, checked_out_branch);
            list_box.append(&row);
        }
    }

    // Add Tags section
    if !tags.is_empty() {
        let header = create_section_header("Tags");
        list_box.append(&header);

        let sorted_tags = sort_tags(tags);
        for tag_info in &sorted_tags {
            let row = create_tag_row(tag_info, checked_out_tag);
            list_box.append(&row);
        }
    }
}

/// Create a section header row (non-selectable).
fn create_section_header(title: &str) -> gtk::ListBoxRow {
    let label = gtk::Label::builder()
        .label(title)
        .halign(gtk::Align::Start)
        .margin_start(12)
        .margin_top(12)
        .margin_bottom(4)
        .build();
    label.add_css_class("heading");
    label.add_css_class("dim-label");

    let row = gtk::ListBoxRow::new();
    row.set_widget_name("section-header");
    row.set_child(Some(&label));
    row.set_selectable(false);
    row.set_activatable(false);
    row
}

/// Extract the ref name (branch or tag) from a list box row.
fn row_ref_name(row: &gtk::ListBoxRow) -> Option<String> {
    let child = row.child()?;
    let box_widget = child.downcast_ref::<gtk::Box>()?;
    let mut current = box_widget.first_child();
    while let Some(widget) = current {
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            let name = label.widget_name();
            if name == "branch-name" || name == "tag-name" {
                return Some(label.text().to_string());
            }
        }
        current = widget.next_sibling();
    }
    None
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

/// Sort tags alphabetically using natural sort.
fn sort_tags(tags: &[TagInfo]) -> Vec<TagInfo> {
    let mut sorted = tags.to_vec();
    sorted.sort_by(|a, b| natural_compare(&a.name, &b.name));
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

    // Checked-out branch indicator (keeps its space; opacity toggled).
    let is_checked_out = checked_out_branch.is_some_and(|b| b == branch_info.name);
    let check_icon = gtk::Image::from_icon_name("object-select-symbolic");
    check_icon.set_pixel_size(16);
    check_icon.set_opacity(if is_checked_out { 1.0 } else { 0.0 });
    check_icon.set_tooltip_text(Some("Checked out branch"));
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
fn create_tag_row(tag_info: &TagInfo, checked_out_tag: Option<&str>) -> gtk::ListBoxRow {
    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .spacing(8)
        .build();

    // Checked-out tag indicator (keeps its space; opacity toggled).
    let is_checked_out = checked_out_tag.is_some_and(|t| t == tag_info.name);
    let check_icon = gtk::Image::from_icon_name("object-select-symbolic");
    check_icon.set_pixel_size(16);
    check_icon.set_opacity(if is_checked_out { 1.0 } else { 0.0 });
    check_icon.set_tooltip_text(Some("Checked out tag"));
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
