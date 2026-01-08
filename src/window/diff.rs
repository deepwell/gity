use gtk::{
    glib::{self, BoxedAnyObject},
    prelude::*,
};
use sourceview5 as sv;
use std::cell::Ref;
use std::sync::mpsc;
use sv::prelude::*;

use crate::git;

use super::state::AppState;
use super::ui::WindowUi;

// Performance tuning:
// - Huge diffs can contain many files and many lines. Building a TextView/SourceView pair for
//   every file (and expanding them all) makes any resize of the surrounding layout very costly
//   because GTK has to relayout + repaint all of that content continuously while the user drags.
// - We default to expanding only the first file and lazily create/destroy per-file widgets when
//   the expander is opened/closed.
const DEFAULT_EXPANDED_FILES: usize = 10;

fn copy_text_to_clipboard(text: &str) {
    // Best-effort: if there's no display (headless/tests), do nothing.
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    display.clipboard().set_text(text);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLineKind {
    Add,
    Remove,
    Context,
    Hunk,
    Header,
    Other,
}

#[derive(Debug)]
struct PreparedDiffSection {
    label: String,
    gutter_text: String,
    right_text: String,
    kinds: Vec<DiffLineKind>,
    gutter_chars: usize,
}

fn clear_container(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn set_placeholder(container: &gtk::Box, text: &str) {
    clear_container(container);
    container.append(
        &gtk::Label::builder()
            .label(text)
            .halign(gtk::Align::Start)
            .wrap(true)
            .build(),
    );
}

fn diff_has_any_collapsed_file(diff_files_box: &gtk::Box) -> Option<bool> {
    // Returns:
    // - None: there are no file expanders (no diff loaded / placeholder)
    // - Some(true): at least one expander exists and is collapsed
    // - Some(false): at least one expander exists and all are expanded
    let mut saw_any = false;
    let mut child = diff_files_box.first_child();
    while let Some(w) = child {
        let next = w.next_sibling();
        if let Ok(expander) = w.downcast::<gtk::Expander>() {
            saw_any = true;
            if !expander.is_expanded() {
                return Some(true);
            }
        }
        child = next;
    }
    if saw_any {
        Some(false)
    } else {
        None
    }
}

fn update_expand_toggle_button(diff_files_box: &gtk::Box, toggle_button: &gtk::Button) {
    match diff_has_any_collapsed_file(diff_files_box) {
        None => {
            toggle_button.set_sensitive(false);
            toggle_button.set_label("Expand all");
        }
        Some(any_collapsed) => {
            toggle_button.set_sensitive(true);
            if any_collapsed {
                toggle_button.set_label("Expand all");
            } else {
                toggle_button.set_label("Collapse all");
            }
        }
    }
}

fn set_all_file_expanders(diff_files_box: &gtk::Box, expanded: bool) {
    let mut child = diff_files_box.first_child();
    while let Some(w) = child {
        let next = w.next_sibling();
        if let Ok(expander) = w.downcast::<gtk::Expander>() {
            expander.set_expanded(expanded);
        }
        child = next;
    }
}

#[derive(Debug)]
struct DiffSection {
    label: String,
    text: String,
}

fn parse_diff_sections(diff: &str) -> Vec<DiffSection> {
    let mut sections: Vec<DiffSection> = Vec::new();
    let mut current_label: Option<String> = None;
    let mut current_lines: Vec<&str> = Vec::new();

    for line in diff.lines() {
        if line.starts_with("diff --git ") {
            if let Some(label) = current_label.take() {
                sections.push(DiffSection {
                    label,
                    text: current_lines.join("\n") + "\n",
                });
                current_lines.clear();
            }

            // Example: diff --git a/foo/bar.rs b/foo/bar.rs
            let mut label = "Diff".to_string();
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 4 {
                let b_path = parts[3].trim_start_matches("b/");
                if !b_path.is_empty() {
                    label = b_path.to_string();
                }
            }

            current_label = Some(label);
            // Don't include the diff header line itself in the displayed text.
            continue;
        }

        // Hide noisy patch header lines inside each file section.
        if line.starts_with("index ") || line.starts_with("---") || line.starts_with("+++") {
            continue;
        }

        current_lines.push(line);
    }

    if let Some(label) = current_label.take() {
        sections.push(DiffSection {
            label,
            text: current_lines.join("\n") + "\n",
        });
    }

    if sections.is_empty() && !diff.trim().is_empty() {
        sections.push(DiffSection {
            label: "Diff".to_string(),
            text: diff.to_string(),
        });
    }

    sections
}

fn apply_basic_diff_line_tags(buffer: &gtk::TextBuffer) {
    let tag_table = buffer.tag_table();

    // Tags are best-effort: if they already exist, re-use them.
    let add_tag = tag_table.lookup("diff-add").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("diff-add"));
        tag_table.add(&tag);
        tag
    });
    // Use paragraph background so the highlight spans the whole line to the right edge,
    // not just behind the glyphs.
    add_tag.set_property("paragraph-background", "#EBFCEC");
    add_tag.set_property("paragraph-background-set", true);
    add_tag.set_property("weight-set", true);

    let remove_tag = tag_table.lookup("diff-remove").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("diff-remove"));
        tag_table.add(&tag);
        tag
    });
    remove_tag.set_property("paragraph-background", "#ffebee");
    remove_tag.set_property("paragraph-background-set", true);
    remove_tag.set_property("weight-set", true);

    let hunk_tag = tag_table.lookup("diff-hunk").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("diff-hunk"));
        tag.set_property("foreground", "#1565c0"); // blue-ish
        tag.set_property("weight-set", true);
        tag_table.add(&tag);
        tag
    });

    let header_tag = tag_table.lookup("diff-header").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("diff-header"));
        tag.set_property("foreground", "#6a1b9a"); // purple-ish
        tag.set_property("weight-set", true);
        tag_table.add(&tag);
        tag
    });

    let start = buffer.start_iter();
    let end = buffer.end_iter();
    buffer.remove_all_tags(&start, &end);

    let mut line_start = buffer.start_iter();
    while !line_start.is_end() {
        let mut line_end = line_start.clone();
        line_end.forward_to_line_end();

        let text = buffer.text(&line_start, &line_end, false);
        let line = text.as_str();

        // Avoid tagging the file header lines as additions/deletions.
        if line.starts_with("@@") {
            buffer.apply_tag(&hunk_tag, &line_start, &line_end);
        } else if line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("---")
            || line.starts_with("+++")
        {
            buffer.apply_tag(&header_tag, &line_start, &line_end);
        } else if line.starts_with('+') && !line.starts_with("+++") {
            buffer.apply_tag(&add_tag, &line_start, &line_end);
        } else if line.starts_with('-') && !line.starts_with("---") {
            buffer.apply_tag(&remove_tag, &line_start, &line_end);
        }

        // Move to next line.
        if !line_start.forward_line() {
            break;
        }
    }
}

fn parse_hunk_header(line: &str) -> Option<(i64, i64)> {
    // Example: @@ -12,7 +34,8 @@ optional heading
    if !line.starts_with("@@") {
        return None;
    }
    let parts: Vec<&str> = line.split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }
    let old_part = parts[1].strip_prefix('-')?;
    let new_part = parts[2].strip_prefix('+')?;
    let old_start = old_part.split(',').next()?.parse::<i64>().ok()?;
    let new_start = new_part.split(',').next()?.parse::<i64>().ok()?;
    Some((old_start, new_start))
}

fn build_diff_gutter_and_text(diff_text: &str) -> (String, String, Vec<DiffLineKind>, usize) {
    let mut old_line: Option<i64> = None;
    let mut new_line: Option<i64> = None;

    let mut max_old: i64 = 0;
    let mut max_new: i64 = 0;

    let mut gutter_lines: Vec<(Option<i64>, Option<i64>, char)> = Vec::new();
    let mut right_lines: Vec<String> = Vec::new();
    let mut kinds: Vec<DiffLineKind> = Vec::new();

    for line in diff_text.lines() {
        // Default classification based on unified diff conventions.
        if let Some((o, n)) = parse_hunk_header(line) {
            old_line = Some(o);
            new_line = Some(n);
            gutter_lines.push((None, None, ' '));
            right_lines.push(line.to_string());
            kinds.push(DiffLineKind::Hunk);
            continue;
        }

        // These normally aren't present (we filter some earlier), but keep safe tagging.
        if line.starts_with("diff ")
            || line.starts_with("index ")
            || line.starts_with("---")
            || line.starts_with("+++")
        {
            gutter_lines.push((None, None, ' '));
            right_lines.push(line.to_string());
            kinds.push(DiffLineKind::Header);
            continue;
        }

        let mut kind = DiffLineKind::Other;
        let mut sym = ' ';
        let mut o = None;
        let mut n = None;

        let right = if let Some(rest) = line.strip_prefix('+') {
            kind = DiffLineKind::Add;
            sym = '+';
            n = new_line;
            if let Some(v) = &mut new_line {
                *v += 1;
            }
            rest.to_string()
        } else if let Some(rest) = line.strip_prefix('-') {
            kind = DiffLineKind::Remove;
            sym = '-';
            o = old_line;
            if let Some(v) = &mut old_line {
                *v += 1;
            }
            rest.to_string()
        } else if let Some(rest) = line.strip_prefix(' ') {
            kind = DiffLineKind::Context;
            sym = ' ';
            o = old_line;
            n = new_line;
            if let Some(v) = &mut old_line {
                *v += 1;
            }
            if let Some(v) = &mut new_line {
                *v += 1;
            }
            rest.to_string()
        } else {
            // Lines like "\ No newline at end of file" shouldn't advance counters.
            line.to_string()
        };

        if let Some(v) = o {
            max_old = max_old.max(v);
        }
        if let Some(v) = n {
            max_new = max_new.max(v);
        }

        gutter_lines.push((o, n, sym));
        right_lines.push(right);
        kinds.push(kind);
    }

    let width = max_old.max(max_new).to_string().len().max(1);

    // Characters per gutter line: "<old> <new> <sym>"
    // old/new are both right-aligned to `width`.
    let gutter_chars = width * 2 + 3;

    let mut gutter = String::new();
    for (idx, (o, n, sym)) in gutter_lines.into_iter().enumerate() {
        if idx > 0 {
            gutter.push('\n');
        }
        let o_s = o.map(|v| format!("{:>width$}", v, width = width));
        let n_s = n.map(|v| format!("{:>width$}", v, width = width));

        // Format: " old new sym"
        // Keep a little breathing room between columns for readability.
        gutter.push_str(o_s.as_deref().unwrap_or(&" ".repeat(width)));
        gutter.push(' ');
        gutter.push_str(n_s.as_deref().unwrap_or(&" ".repeat(width)));
        gutter.push(' ');
        gutter.push(sym);
    }

    (
        gutter + "\n",
        right_lines.join("\n") + "\n",
        kinds,
        gutter_chars,
    )
}

fn apply_diff_line_tags_by_kind(buffer: &gtk::TextBuffer, kinds: &[DiffLineKind]) {
    // Re-use the existing tag definitions (colors etc.)
    apply_basic_diff_line_tags(buffer);

    // But if we stripped the +/- prefixes, the old tagger won't find them anymore.
    // So clear and apply tags explicitly by line kind.
    let tag_table = buffer.tag_table();
    let add_tag = tag_table.lookup("diff-add");
    let remove_tag = tag_table.lookup("diff-remove");
    let hunk_tag = tag_table.lookup("diff-hunk");
    let header_tag = tag_table.lookup("diff-header");

    let start = buffer.start_iter();
    let end = buffer.end_iter();
    buffer.remove_all_tags(&start, &end);

    for (idx, kind) in kinds.iter().enumerate() {
        let Some(line_start) = buffer.iter_at_line(idx as i32) else {
            break;
        };
        let mut line_end = line_start.clone();
        line_end.forward_to_line_end();
        match kind {
            DiffLineKind::Add => {
                if let Some(tag) = &add_tag {
                    buffer.apply_tag(tag, &line_start, &line_end);
                }
            }
            DiffLineKind::Remove => {
                if let Some(tag) = &remove_tag {
                    buffer.apply_tag(tag, &line_start, &line_end);
                }
            }
            DiffLineKind::Hunk => {
                if let Some(tag) = &hunk_tag {
                    buffer.apply_tag(tag, &line_start, &line_end);
                }
            }
            DiffLineKind::Header => {
                if let Some(tag) = &header_tag {
                    buffer.apply_tag(tag, &line_start, &line_end);
                }
            }
            _ => {}
        }
    }
}

fn build_file_row(prepared: &PreparedDiffSection, global_gutter_chars: usize) -> gtk::Box {
    let gutter_text = &prepared.gutter_text;
    let right_text = &prepared.right_text;
    let kinds = &prepared.kinds;

    // Left: gutter
    let gutter_buffer = gtk::TextBuffer::new(None);
    gutter_buffer.set_text(gutter_text);

    let gutter_view = gtk::TextView::with_buffer(&gutter_buffer);
    gutter_view.set_editable(false);
    gutter_view.set_cursor_visible(false);
    gutter_view.set_monospace(true);
    gutter_view.set_wrap_mode(gtk::WrapMode::None);
    gutter_view.set_can_focus(false);
    gutter_view.set_hexpand(false);
    gutter_view.set_pixels_above_lines(0);
    gutter_view.set_pixels_below_lines(0);
    gutter_view.set_top_margin(0);
    gutter_view.set_bottom_margin(0);
    gutter_view.set_left_margin(8);
    gutter_view.set_right_margin(0);

    // Size the gutter to the minimum width needed to show line numbers and symbols.
    // (Non-expanding, so the right pane takes the remaining space.)
    let probe = "0".repeat(global_gutter_chars.max(1));
    let layout = gutter_view.create_pango_layout(Some(&probe));
    let (probe_px, _) = layout.pixel_size();
    gutter_view.set_width_request(probe_px + 8); // + left padding
    gutter_view.add_css_class("diff-gutter");

    // Right: diff text (single editable component)
    let buffer = sv::Buffer::new(None);
    // We do our own line coloring, so keep syntax highlighting off for consistent results.
    buffer.set_highlight_syntax(false);
    buffer.set_text(right_text);

    apply_diff_line_tags_by_kind(buffer.upcast_ref::<gtk::TextBuffer>(), &kinds);

    let view = sv::View::with_buffer(&buffer);
    // Diff is a viewer: keep it read-only to avoid IME/input-method paths on Wayland
    // that can emit Gtk warnings (and we still get selection/copy).
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_can_focus(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::None);
    view.set_show_line_numbers(false);
    view.set_highlight_current_line(false);
    view.set_hexpand(true);
    view.set_pixels_above_lines(0);
    view.set_pixels_below_lines(0);
    view.set_top_margin(0);
    view.set_bottom_margin(0);
    view.set_left_margin(0);
    view.set_right_margin(0);

    // Horizontal scrolling (per file):
    // The outer diff panel already provides vertical scrolling. Wrapping each file's text view in a
    // scrolled window with vertical scrolling disabled gives us a performant horizontal scrollbar
    // without introducing nested vertical scrolling.
    let h_scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Automatic)
        .vscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .build();
    h_scroller.set_hexpand(true);
    h_scroller.set_child(Some(&view));

    // Two-widget layout: gutter on the left, text on the right
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .build();
    row.set_spacing(8);
    row.append(&gutter_view);
    row.append(&h_scroller);

    row
}

fn build_file_expander_lazy(
    prepared: &PreparedDiffSection,
    expanded: bool,
    global_gutter_chars: usize,
) -> gtk::Expander {
    let expander = gtk::Expander::builder().expanded(expanded).build();

    // Custom expander label: filename + copy-to-clipboard icon button.
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .build();
    header.set_halign(gtk::Align::Start);

    let filename_label = gtk::Label::builder()
        .label(&prepared.label)
        .xalign(0.0)
        .build();
    filename_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);

    // Use a Stack to animate between copy and success icons
    let copy_icon = gtk::Image::from_icon_name("edit-copy-symbolic");
    copy_icon.set_pixel_size(14);

    let success_icon = gtk::Image::from_icon_name("object-select-symbolic");
    success_icon.set_pixel_size(14);

    let icon_stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .transition_duration(200)
        .build();
    icon_stack.add_named(&copy_icon, Some("copy"));
    icon_stack.add_named(&success_icon, Some("success"));
    icon_stack.set_visible_child_name("copy");

    let copy_button = gtk::Button::builder()
        .child(&icon_stack)
        .tooltip_text("Copy filename")
        .valign(gtk::Align::Center)
        .build();
    copy_button.add_css_class("flat");
    copy_button.add_css_class("copy-filename-btn");
    copy_button.set_can_focus(false);
    copy_button.set_width_request(22);
    copy_button.set_height_request(22);

    // Show copy button on hover over the header row
    let motion_controller = gtk::EventControllerMotion::new();
    let copy_button_for_enter = copy_button.clone();
    motion_controller.connect_enter(move |_, _, _| {
        copy_button_for_enter.add_css_class("visible");
    });
    let copy_button_for_leave = copy_button.clone();
    motion_controller.connect_leave(move |_| {
        if !copy_button_for_leave.has_css_class("success") {
            copy_button_for_leave.remove_css_class("visible");
        }
    });
    header.add_controller(motion_controller);

    let filename_for_copy = prepared.label.clone();
    let icon_stack_for_click = icon_stack.clone();
    let copy_button_for_click = copy_button.clone();
    copy_button.connect_clicked(move |_| {
        copy_text_to_clipboard(&filename_for_copy);

        icon_stack_for_click.set_visible_child_name("success");
        copy_button_for_click.add_css_class("success");

        let stack_clone = icon_stack_for_click.clone();
        let button_clone = copy_button_for_click.clone();
        glib::timeout_add_local_once(std::time::Duration::from_millis(1500), move || {
            button_clone.remove_css_class("success");
            button_clone.remove_css_class("visible");

            let stack_inner = stack_clone.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(150), move || {
                stack_inner.set_visible_child_name("copy");
            });
        });
    });

    // Spacer eats remaining header width so the copy button stays immediately after the label,
    // rather than aligning to the far right.
    let spacer = gtk::Box::builder().hexpand(true).build();

    header.append(&filename_label);
    header.append(&copy_button);
    header.append(&spacer);
    expander.set_label_widget(Some(&header));

    // If expanded initially, build the heavy child once now.
    if expanded {
        let row = build_file_row(prepared, global_gutter_chars);
        expander.set_child(Some(&row));
    }

    // Lazy-build / drop the heavy child as the user opens/closes the file section.
    // This keeps the widget tree lightweight during paned/window resizing.
    let prepared_for_cb = PreparedDiffSection {
        label: prepared.label.clone(),
        gutter_text: prepared.gutter_text.clone(),
        right_text: prepared.right_text.clone(),
        kinds: prepared.kinds.clone(),
        gutter_chars: prepared.gutter_chars,
    };

    expander.connect_expanded_notify(move |exp| {
        if exp.is_expanded() {
            if exp.child().is_none() {
                let row = build_file_row(&prepared_for_cb, global_gutter_chars);
                exp.set_child(Some(&row));
            }
        } else {
            // Drop large buffers/views when collapsed to reduce relayout/repaint costs elsewhere.
            exp.set_child(None::<&gtk::Widget>);
        }
    });

    expander
}

// Helper function to poll channel and update diff UI
fn poll_diff_result(
    rx: mpsc::Receiver<Result<String, git2::Error>>,
    diff_files_box: gtk::Box,
    toggle_button: gtk::Button,
) {
    match rx.try_recv() {
        Ok(Ok(diff)) => {
            let sections = parse_diff_sections(&diff);
            clear_container(&diff_files_box);

            if sections.is_empty() {
                set_placeholder(&diff_files_box, "");
                update_expand_toggle_button(&diff_files_box, &toggle_button);
                return;
            }

            let mut prepared_sections: Vec<PreparedDiffSection> =
                Vec::with_capacity(sections.len());
            for section in sections {
                let (gutter_text, right_text, kinds, gutter_chars) =
                    build_diff_gutter_and_text(&section.text);
                prepared_sections.push(PreparedDiffSection {
                    label: section.label,
                    gutter_text,
                    right_text,
                    kinds,
                    gutter_chars,
                });
            }

            // Make the gutter width consistent across all files in this commit diff.
            let global_gutter_chars = prepared_sections
                .iter()
                .map(|s| s.gutter_chars)
                .max()
                .unwrap_or(1);

            for (idx, prepared) in prepared_sections.iter().enumerate() {
                // Expand only the first file by default to keep huge diffs responsive.
                let expanded = idx < DEFAULT_EXPANDED_FILES;
                let expander = build_file_expander_lazy(prepared, expanded, global_gutter_chars);
                // Keep the toggle button label in sync if the user expands/collapses individual files.
                let diff_files_box_for_notify = diff_files_box.clone();
                let toggle_for_notify = toggle_button.clone();
                expander.connect_expanded_notify(move |_| {
                    update_expand_toggle_button(&diff_files_box_for_notify, &toggle_for_notify);
                });
                diff_files_box.append(&expander);
            }

            update_expand_toggle_button(&diff_files_box, &toggle_button);
        }
        Ok(Err(e)) => {
            let error_msg = format!("Error loading diff: {}", e);
            set_placeholder(&diff_files_box, &error_msg);
            update_expand_toggle_button(&diff_files_box, &toggle_button);
        }
        Err(mpsc::TryRecvError::Empty) => {
            let diff_files_box_clone = diff_files_box.clone();
            let toggle_btn_clone = toggle_button.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
                poll_diff_result(rx, diff_files_box_clone, toggle_btn_clone);
            });
        }
        Err(_) => {
            set_placeholder(&diff_files_box, "Error: channel closed");
            update_expand_toggle_button(&diff_files_box, &toggle_button);
        }
    }
}

// Helper to truncate message to first N lines
fn truncate_to_lines(text: &str, max_lines: usize) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let has_more = lines.len() > max_lines;
    let truncated = if has_more {
        lines[..max_lines].join("\n")
    } else {
        text.to_string()
    };
    (truncated, has_more)
}

// Helper function to poll metadata channel and update labels
fn poll_metadata_result(
    rx: mpsc::Receiver<Result<git::CommitMetadata, git2::Error>>,
    diff_label: gtk::Label,
    commit_message_label: gtk::Label,
    expand_label: gtk::Label,
    full_message: std::rc::Rc<std::cell::RefCell<String>>,
    is_expanded: std::rc::Rc<std::cell::RefCell<bool>>,
) {
    match rx.try_recv() {
        Ok(Ok(metadata)) => {
            let label_text = format!(
                "{} <{}> - {} - {}",
                metadata.author_name, metadata.author_email, metadata.date_time, metadata.git_sha
            );
            diff_label.set_text(&label_text);

            *full_message.borrow_mut() = metadata.commit_message.clone();
            *is_expanded.borrow_mut() = false;

            let (truncated, has_more) = truncate_to_lines(&metadata.commit_message, 5);
            commit_message_label.set_text(&truncated);
            expand_label.set_visible(has_more);
            if has_more {
                expand_label.set_markup("<b>Show more</b>");
            }
        }
        Ok(Err(_)) => {
            // On error, keep default label
        }
        Err(mpsc::TryRecvError::Empty) => {
            let diff_label_clone = diff_label.clone();
            let commit_message_label_clone = commit_message_label.clone();
            let expand_label_clone = expand_label.clone();
            let full_message_clone = full_message.clone();
            let is_expanded_clone = is_expanded.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
                poll_metadata_result(
                    rx,
                    diff_label_clone,
                    commit_message_label_clone,
                    expand_label_clone,
                    full_message_clone,
                    is_expanded_clone,
                );
            });
        }
        Err(_) => {
            // Channel closed, keep default label
        }
    }
}

fn load_commit_diff(ui: &WindowUi, state: &AppState, commit_sha: &str) {
    if let Some(ref path) = *state.current_path.borrow() {
        // Show loading message
        set_placeholder(&ui.repo_view.diff_files_box, "Loading diff...");
        ui.repo_view.diff_label.set_text("Loading author...");
        ui.repo_view.commit_message_label.set_text("");
        ui.repo_view.expand_label.set_visible(false);
        *ui.repo_view.is_expanded.borrow_mut() = false;
        update_expand_toggle_button(
            &ui.repo_view.diff_files_box,
            &ui.repo_view.diff_expand_toggle_button,
        );

        // Load diff in background thread
        let diff_files_box_clone = ui.repo_view.diff_files_box.clone();
        let toggle_btn = ui.repo_view.diff_expand_toggle_button.clone();
        let path_clone = path.clone();
        let sha_clone = commit_sha.to_string();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let diff_result = git::get_commit_diff(path_clone.to_str().unwrap(), &sha_clone);
            let _ = tx.send(diff_result);
        });
        poll_diff_result(rx, diff_files_box_clone, toggle_btn);

        // Load metadata in background thread
        let diff_label_clone = ui.repo_view.diff_label.clone();
        let commit_message_label_clone = ui.repo_view.commit_message_label.clone();
        let expand_label_clone = ui.repo_view.expand_label.clone();
        let full_message_clone = ui.repo_view.full_message.clone();
        let is_expanded_clone = ui.repo_view.is_expanded.clone();
        let path_clone_meta = path.clone();
        let sha_clone_meta = commit_sha.to_string();
        let (tx_meta, rx_meta) = mpsc::channel();
        std::thread::spawn(move || {
            let metadata_result =
                git::get_commit_metadata(path_clone_meta.to_str().unwrap(), &sha_clone_meta);
            let _ = tx_meta.send(metadata_result);
        });
        poll_metadata_result(
            rx_meta,
            diff_label_clone,
            commit_message_label_clone,
            expand_label_clone,
            full_message_clone,
            is_expanded_clone,
        );
    } else {
        set_placeholder(&ui.repo_view.diff_files_box, "No repository loaded");
        ui.repo_view.diff_label.set_text("Commit Diff");
        ui.repo_view.commit_message_label.set_text("");
        ui.repo_view.expand_label.set_visible(false);
        *ui.repo_view.is_expanded.borrow_mut() = false;
        update_expand_toggle_button(
            &ui.repo_view.diff_files_box,
            &ui.repo_view.diff_expand_toggle_button,
        );
    }
}

pub fn connect(ui: &WindowUi, state: &AppState) {
    // Wire diff header controls
    let diff_files_box_for_toggle = ui.repo_view.diff_files_box.clone();
    let toggle_for_click = ui.repo_view.diff_expand_toggle_button.clone();
    ui.repo_view
        .diff_expand_toggle_button
        .connect_clicked(move |_| {
            let any_collapsed =
                diff_has_any_collapsed_file(&diff_files_box_for_toggle).unwrap_or(true);
            set_all_file_expanders(&diff_files_box_for_toggle, any_collapsed);
            update_expand_toggle_button(&diff_files_box_for_toggle, &toggle_for_click);
        });

    let ui_for_selection = ui.clone();
    let state_for_selection = state.clone();

    ui.repo_view
        .commit_list
        .selection_model
        .connect_selected_item_notify(move |model: &gtk::SingleSelection| {
            let selected_item = model.selected_item();
            if let Some(item) = selected_item {
                if let Ok(commit_obj) = item.downcast::<BoxedAnyObject>() {
                    let commit_ref: Ref<git::GitCommit> = commit_obj.borrow();
                    let commit_sha = commit_ref.id.clone();
                    load_commit_diff(&ui_for_selection, &state_for_selection, &commit_sha);
                }
            } else {
                set_placeholder(&ui_for_selection.repo_view.diff_files_box, "");
                ui_for_selection
                    .repo_view
                    .diff_label
                    .set_text("Commit Diff");
                ui_for_selection.repo_view.commit_message_label.set_text("");
                ui_for_selection.repo_view.expand_label.set_visible(false);
                *ui_for_selection.repo_view.is_expanded.borrow_mut() = false;
                update_expand_toggle_button(
                    &ui_for_selection.repo_view.diff_files_box,
                    &ui_for_selection.repo_view.diff_expand_toggle_button,
                );
            }
        });
}
