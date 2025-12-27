//! Branch panel UI component for displaying and selecting git branches.
//!
//! This module provides the `BranchPanel` widget which displays a list of
//! git branches with their last commit time and allows single-selection.

use chrono::{DateTime, Utc};
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use crate::git::BranchInfo;

/// A panel widget that displays a list of git branches.
///
/// The panel shows each branch with:
/// - A checkmark icon for the currently checked-out branch
/// - The branch name (with ellipsis for long names)
/// - Relative time since last commit
///
/// Branches are sorted with "main" first, then alphabetically using
/// natural sort order (e.g., "branch-2" comes before "branch-10").
#[derive(Clone)]
pub struct BranchPanel {
    /// The root widget container
    pub widget: gtk::Box,
    /// The list box containing branch rows
    list_box: gtk::ListBox,
    /// Currently selected branch name
    selected_branch: Rc<RefCell<Option<String>>>,
}

impl BranchPanel {
    /// Create a new BranchPanel with the given branches.
    ///
    /// # Arguments
    /// * `branches` - Slice of branch information to display
    pub fn new(branches: &[BranchInfo]) -> Self {
        Self::new_with_checked_out_branch(branches, None)
    }

    /// Create a new BranchPanel with branches and indication of which is checked out.
    ///
    /// # Arguments
    /// * `branches` - Slice of branch information to display
    /// * `checked_out_branch` - Name of the currently checked out branch (if any)
    pub fn new_with_checked_out_branch(
        branches: &[BranchInfo],
        checked_out_branch: Option<&str>,
    ) -> Self {
        let side_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();

        // Create a scrolled window for the branch list
        let scrolled = gtk::ScrolledWindow::builder().vexpand(true).build();

        // Create a list box to hold branches
        let list_box = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .build();

        let sorted_branches = sort_branches(branches);

        // Add each branch to the list
        for branch_info in &sorted_branches {
            let row = create_branch_row(branch_info, checked_out_branch);
            list_box.append(&row);
        }

        scrolled.set_child(Some(&list_box));
        side_panel.append(&scrolled);
        side_panel.set_vexpand(true);

        let panel = Self {
            widget: side_panel,
            list_box,
            selected_branch: Rc::new(RefCell::new(None)),
        };

        // Default selection: prefer checked-out branch, then "main", then first row.
        if let Some(name) = checked_out_branch {
            let _ = panel.select_branch(name);
        }
        panel.ensure_default_selection();
        panel
    }

    /// Update the panel with a new list of branches.
    ///
    /// Attempts to preserve the current selection if possible.
    ///
    /// # Arguments
    /// * `branches` - New slice of branch information
    /// * `checked_out_branch` - Name of the currently checked out branch (if any)
    pub fn update_branches(&self, branches: &[BranchInfo], checked_out_branch: Option<&str>) {
        // Preserve the last selected branch (or current selected row if present).
        let preserved = self.selected_branch.borrow().clone().or_else(|| {
            self.list_box
                .selected_row()
                .and_then(|r| row_branch_name(&r))
        });

        // Clear existing branches
        while let Some(row) = self.list_box.row_at_index(0) {
            self.list_box.remove(&row);
        }

        let sorted_branches = sort_branches(branches);

        // Add new branches
        for branch_info in &sorted_branches {
            let row = create_branch_row(branch_info, checked_out_branch);
            self.list_box.append(&row);
        }

        if let Some(name) = preserved {
            let _ = self.select_branch(&name);
        } else if let Some(name) = checked_out_branch {
            let _ = self.select_branch(name);
        }
        self.ensure_default_selection();
    }

    /// Select a branch by name.
    ///
    /// # Arguments
    /// * `branch_name` - Name of the branch to select
    ///
    /// # Returns
    /// `true` if the branch was found and selected, `false` otherwise.
    pub fn select_branch(&self, branch_name: &str) -> bool {
        let mut i = 0;
        while let Some(row) = self.list_box.row_at_index(i) {
            if let Some(name) = row_branch_name(&row) {
                if name == branch_name {
                    self.list_box.select_row(Some(&row));
                    *self.selected_branch.borrow_mut() = Some(branch_name.to_string());
                    return true;
                }
            }
            i += 1;
        }
        false
    }

    /// Register a callback for when a branch is selected (activated).
    ///
    /// # Arguments
    /// * `callback` - Function called with the branch name when a branch is activated
    pub fn branch_selected<F: Fn(&str) + 'static>(&self, callback: F) {
        let selected_branch = self.selected_branch.clone();
        self.list_box.connect_row_activated(move |_, row| {
            if let Some(branch_name) = row_branch_name(row) {
                *selected_branch.borrow_mut() = Some(branch_name.clone());
                callback(&branch_name);
            }
        });
    }

    /// Ensure a default branch is selected if nothing is currently selected.
    fn ensure_default_selection(&self) {
        if self.list_box.selected_row().is_some() {
            return;
        }

        // Prefer main if present.
        if self.select_branch("main") {
            return;
        }

        // Otherwise select first row if present.
        if let Some(row) = self.list_box.row_at_index(0) {
            self.list_box.select_row(Some(&row));
            if let Some(name) = row_branch_name(&row) {
                *self.selected_branch.borrow_mut() = Some(name);
            }
        }
    }
}

// =============================================================================
// Private helper functions
// =============================================================================

/// Extract the branch name from a list box row.
fn row_branch_name(row: &gtk::ListBoxRow) -> Option<String> {
    let child = row.child()?;
    let box_widget = child.downcast_ref::<gtk::Box>()?;
    let mut current = box_widget.first_child();
    while let Some(widget) = current {
        if let Some(label) = widget.downcast_ref::<gtk::Label>() {
            if label.widget_name() == "branch-name" {
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
