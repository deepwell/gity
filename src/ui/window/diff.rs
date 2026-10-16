use gtk::{glib, prelude::*};
use sourceview5 as sv;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use sv::prelude::*;

use crate::git;
use crate::ui::image_diff::{self, ImageDiffSource};
use crate::ui::{DiffFileTree, FileSummary, copy_on_hover};

use super::state::AppState;
use super::ui::WindowUi;

/// Colors used for diff highlighting
struct DiffColors {
    add_bg: &'static str,
    remove_bg: &'static str,
    gutter_add_bg: &'static str,
    gutter_remove_bg: &'static str,
    hunk_fg: &'static str,
    header_fg: &'static str,
}

/// Light mode diff colors
const DIFF_COLORS_LIGHT: DiffColors = DiffColors {
    add_bg: "#EBFCEC",
    remove_bg: "#ffebee",
    gutter_add_bg: "#c1f7c2",    // Darker green for gutter
    gutter_remove_bg: "#ffcdd2", // Darker red for gutter
    hunk_fg: "#1565c0",
    header_fg: "#6a1b9a",
};

/// Dark mode diff colors
const DIFF_COLORS_DARK: DiffColors = DiffColors {
    add_bg: "#1a3d1a",
    remove_bg: "#3d1a1a",
    gutter_add_bg: "#2e5a2e",    // Brighter green for gutter
    gutter_remove_bg: "#5a2e2e", // Brighter red for gutter
    hunk_fg: "#64b5f6",
    header_fg: "#ce93d8",
};

/// Get diff colors based on current color scheme (light/dark)
fn get_diff_colors() -> &'static DiffColors {
    if adw::StyleManager::default().is_dark() {
        &DIFF_COLORS_DARK
    } else {
        &DIFF_COLORS_LIGHT
    }
}

/// Update existing diff tag colors without re-applying tags to text.
/// This is used when the theme changes to refresh colors in-place.
fn update_diff_tag_colors(buffer: &gtk::TextBuffer) {
    let tag_table = buffer.tag_table();
    let colors = get_diff_colors();

    // Code view tags
    if let Some(add_tag) = tag_table.lookup("diff-add") {
        add_tag.set_property("paragraph-background", &colors.add_bg);
    }
    if let Some(remove_tag) = tag_table.lookup("diff-remove") {
        remove_tag.set_property("paragraph-background", &colors.remove_bg);
    }
    if let Some(hunk_tag) = tag_table.lookup("diff-hunk") {
        hunk_tag.set_property("foreground", &colors.hunk_fg);
    }
    if let Some(header_tag) = tag_table.lookup("diff-header") {
        header_tag.set_property("foreground", &colors.header_fg);
    }

    // Gutter-specific tags (darker colors)
    if let Some(gutter_add_tag) = tag_table.lookup("gutter-add") {
        gutter_add_tag.set_property("paragraph-background", &colors.gutter_add_bg);
    }
    if let Some(gutter_remove_tag) = tag_table.lookup("gutter-remove") {
        gutter_remove_tag.set_property("paragraph-background", &colors.gutter_remove_bg);
    }
}

/// CSS for widgets that share the diff line colors: the changed-files "+N" /
/// "−N" chips and the outlines around before/after image previews.
fn diff_colors_css(colors: &DiffColors) -> String {
    format!(
        ".diff-stat-add {{ background-color: {}; }}\n\
         .diff-stat-remove {{ background-color: {}; }}\n\
         .image-diff-after {{ outline: 1px solid {}; }}\n\
         .image-diff-before {{ outline: 1px solid {}; }}\n",
        colors.add_bg, colors.remove_bg, colors.gutter_add_bg, colors.gutter_remove_bg
    )
}

/// Refresh diff colors for all visible diff sections when theme changes.
/// Collapsed sections have no child widget, so only expanded ones are touched.
fn refresh_diff_colors(diff_files_box: &gtk::Box) {
    refresh_text_view_colors(diff_files_box.upcast_ref());
}

/// Updates the diff tag colors of every TextView under `widget`: each file
/// row's gutter and its SourceView (a TextView subclass), however nested.
fn refresh_text_view_colors(widget: &gtk::Widget) {
    if let Some(text_view) = widget.downcast_ref::<gtk::TextView>() {
        update_diff_tag_colors(&text_view.buffer());
        return;
    }
    let mut child = widget.first_child();
    while let Some(c) = child {
        refresh_text_view_colors(&c);
        child = c.next_sibling();
    }
}

// Performance tuning:
// - Huge diffs can contain many files and many lines. Building a TextView/SourceView pair for
//   every file (and expanding them all) makes any resize of the surrounding layout very costly
//   because GTK has to relayout + repaint all of that content continuously while the user drags.
// - We default to expanding only the first file and lazily create/destroy per-file widgets when
//   the expander is opened/closed.
const DEFAULT_EXPANDED_FILES: usize = 10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DiffLineKind {
    Add,
    Remove,
    Context,
    Hunk,
    Header,
    Other,
}

#[derive(Debug, Clone)]
struct PreparedDiffSection {
    label: String,
    gutter_text: String,
    right_text: String,
    kinds: Vec<DiffLineKind>,
    gutter_chars: usize,
    image: Option<ImageDiffSource>,
}

fn clear_container(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

fn set_placeholder(container: &gtk::Box, icon_name: &str, text: &str) {
    clear_container(container);
    container.append(&crate::ui::placeholder::centered(icon_name, text));
}

/// Creates a skeleton line widget for loading states
fn skeleton_line(width_percent: i32, css_class: &str) -> gtk::Box {
    let line = gtk::Box::builder().hexpand(false).build();
    line.add_css_class("skeleton");
    line.add_css_class(css_class);
    // Use width_request as a percentage of typical width
    line.set_width_request(width_percent * 3); // Scale factor for reasonable widths
    line
}

/// Creates a skeleton file section mimicking a diff file expander
fn skeleton_file_section() -> gtk::Box {
    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(10)
        .margin_end(10)
        .margin_top(8)
        .build();

    // File header skeleton
    let header = skeleton_line(80, "skeleton-file-header");
    section.append(&header);

    // Diff content skeleton (a few lines)
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(8)
        .build();
    content.add_css_class("skeleton");
    content.add_css_class("skeleton-diff-block");
    content.set_hexpand(true);
    section.append(&content);

    section
}

/// Shows skeleton loading state for the diff files box
fn set_diff_skeleton(container: &gtk::Box) {
    clear_container(container);
    // Show 3 skeleton file sections to mimic typical diff view
    for _ in 0..3 {
        container.append(&skeleton_file_section());
    }
}

/// Shows skeleton loading state for the commit title, metadata subtitle and body.
fn set_metadata_skeleton(
    title_label: &gtk::Label,
    metadata_label: &gtk::Label,
    avatar_label: &gtk::Label,
    sha_row: &gtk::Box,
    message_label: &gtk::Label,
) {
    title_label.set_text("\u{00A0}");
    title_label.add_css_class("skeleton");
    title_label.set_width_request(400);

    avatar_label.set_text("");
    avatar_label.add_css_class("skeleton");

    metadata_label.set_text("\u{00A0}");
    metadata_label.add_css_class("skeleton");
    metadata_label.set_width_request(300);
    sha_row.set_visible(false);

    // Also show skeleton for commit message
    message_label.set_visible(true);
    message_label.set_text("\u{00A0}\n\u{00A0}\n\u{00A0}");
    message_label.add_css_class("skeleton");
}

/// Removes skeleton styling from metadata labels
fn clear_metadata_skeleton(
    title_label: &gtk::Label,
    metadata_label: &gtk::Label,
    avatar_label: &gtk::Label,
    message_label: &gtk::Label,
) {
    title_label.remove_css_class("skeleton");
    title_label.set_width_request(-1);
    avatar_label.remove_css_class("skeleton");
    metadata_label.remove_css_class("skeleton");
    metadata_label.set_width_request(-1);
    message_label.remove_css_class("skeleton");
}

/// Formats an author as "Name <email>", omitting the email when it is empty.
fn format_author(name: &str, email: &str) -> String {
    if email.is_empty() {
        name.to_string()
    } else {
        format!("{} <{}>", name, email)
    }
}

/// Computes up to two uppercase initials for an author avatar (e.g. "Mark
/// Deepwell" -> "MD", "mark" -> "MA").
fn author_initials(name: &str) -> String {
    let words: Vec<&str> = name.split_whitespace().collect();
    let initials: String = match words.as_slice() {
        [] => String::new(),
        [single] => single.chars().take(2).collect(),
        [first, second, ..] => first
            .chars()
            .next()
            .into_iter()
            .chain(second.chars().next())
            .collect(),
    };
    initials.to_uppercase()
}

/// Returns `(any_expanded, any_collapsed)` across all file expanders.
/// Both are `false` when there are no expanders (no diff loaded / placeholder).
fn diff_expander_state(diff_files_box: &gtk::Box) -> (bool, bool) {
    let mut any_expanded = false;
    let mut any_collapsed = false;
    let mut child = diff_files_box.first_child();
    while let Some(w) = child {
        let next = w.next_sibling();
        if let Ok(expander) = w.downcast::<gtk::Expander>() {
            if expander.is_expanded() {
                any_expanded = true;
            } else {
                any_collapsed = true;
            }
            if any_expanded && any_collapsed {
                break;
            }
        }
        child = next;
    }
    (any_expanded, any_collapsed)
}

fn update_expand_collapse_buttons(
    diff_files_box: &gtk::Box,
    expand_button: &gtk::Button,
    collapse_button: &gtk::Button,
) {
    let (any_expanded, any_collapsed) = diff_expander_state(diff_files_box);
    expand_button.set_sensitive(any_collapsed);
    collapse_button.set_sensitive(any_expanded);
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
    image: Option<ImageDiffSource>,
}

fn file_change_label(file: &git::FileChange) -> String {
    match file.kind {
        git::FileChangeKind::Renamed | git::FileChangeKind::Copied => {
            let old = file.old_path.as_deref().unwrap_or("?");
            let new = file.new_path.as_deref().unwrap_or("?");
            format!("{old} → {new}")
        }
        git::FileChangeKind::Deleted => file
            .old_path
            .clone()
            .or_else(|| file.new_path.clone())
            .unwrap_or_else(|| "Deleted file".to_string()),
        _ => file
            .new_path
            .clone()
            .or_else(|| file.old_path.clone())
            .unwrap_or_else(|| "Diff".to_string()),
    }
}

/// A modification that left the content untouched, so only the file mode
/// (e.g. the executable bit) changed.
fn is_mode_only_change(file: &git::FileChange) -> bool {
    file.kind == git::FileChangeKind::Modified
        && file.old_oid.is_some()
        && file.old_oid == file.new_oid
        && file.old_mode != file.new_mode
}

/// Blob ids to preview when `file` is an image whose content can be shown.
fn image_source(file: &git::FileChange) -> Option<ImageDiffSource> {
    use git::FileChangeKind::*;
    // A mode-only change has nothing visual to compare; it is described as text.
    let previewable = matches!(file.kind, Added | Deleted | Modified | Renamed | Copied)
        && !is_mode_only_change(file);
    let is_image = [&file.old_path, &file.new_path]
        .into_iter()
        .flatten()
        .any(|path| image_diff::is_image_path(path));
    let has_blob = file.old_oid.is_some() || file.new_oid.is_some();
    (previewable && is_image && has_blob).then_some(ImageDiffSource {
        old: file.old_oid,
        new: file.new_oid,
    })
}

fn sections_from_commit_diff(diff: &git::CommitDiff) -> Vec<DiffSection> {
    diff.files
        .iter()
        .map(|file| {
            let label = file_change_label(file);
            let image = image_source(file);
            let text = if is_mode_only_change(file) {
                // Checked before the patch: for binary files libgit2 still
                // emits "Binary files … differ" when only the mode changed.
                format!(
                    "File mode changed from {:o} to {:o} with no content changes\n",
                    u32::from(file.old_mode),
                    u32::from(file.new_mode)
                )
            } else if file.patch.trim().is_empty() {
                match file.kind {
                    git::FileChangeKind::Renamed => "Renamed with no content changes\n".to_string(),
                    git::FileChangeKind::Copied => "Copied with no content changes\n".to_string(),
                    _ => String::new(),
                }
            } else if image.is_some() && file.binary {
                // The preview replaces the "Binary files … differ" line.
                String::new()
            } else {
                file.patch.clone()
            };
            DiffSection { label, text, image }
        })
        .collect()
}

fn file_summaries_from_commit_diff(diff: &git::CommitDiff) -> Vec<FileSummary> {
    diff.files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let (additions, deletions) = file.line_stats();
            FileSummary {
                index,
                path: file
                    .new_path
                    .clone()
                    .or_else(|| file.old_path.clone())
                    .unwrap_or_default(),
                label: file_change_label(file),
                additions,
                deletions,
            }
        })
        .collect()
}

/// Expands the diff section for file `index` and scrolls it to the top of the diff view.
fn scroll_to_file(
    diff_files_box: &gtk::Box,
    diff_scrolled_window: &gtk::ScrolledWindow,
    index: usize,
) {
    let mut child = diff_files_box.first_child();
    let mut expander_index = 0;
    let expander = loop {
        let Some(w) = child else {
            return;
        };
        child = w.next_sibling();
        if let Ok(expander) = w.downcast::<gtk::Expander>() {
            if expander_index == index {
                break expander;
            }
            expander_index += 1;
        }
    };

    expander.set_expanded(true);

    // Wait for the (possibly newly built) section to be laid out before scrolling.
    let diff_files_box = diff_files_box.clone();
    let diff_scrolled_window = diff_scrolled_window.clone();
    glib::idle_add_local_once(move || {
        if let Some(point) = expander.compute_point(&diff_files_box, &gtk::graphene::Point::zero())
        {
            diff_scrolled_window
                .vadjustment()
                .set_value(point.y() as f64);
        }
    });
}

fn apply_basic_diff_line_tags(buffer: &gtk::TextBuffer) {
    let tag_table = buffer.tag_table();
    let colors = get_diff_colors();

    // Tags are best-effort: if they already exist, re-use them.
    let add_tag = tag_table.lookup("diff-add").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("diff-add"));
        tag_table.add(&tag);
        tag
    });
    // Use paragraph background so the highlight spans the whole line to the right edge,
    // not just behind the glyphs.
    add_tag.set_property("paragraph-background", &colors.add_bg);
    add_tag.set_property("paragraph-background-set", true);
    add_tag.set_property("weight-set", true);

    let remove_tag = tag_table.lookup("diff-remove").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("diff-remove"));
        tag_table.add(&tag);
        tag
    });
    remove_tag.set_property("paragraph-background", &colors.remove_bg);
    remove_tag.set_property("paragraph-background-set", true);
    remove_tag.set_property("weight-set", true);

    let hunk_tag = tag_table.lookup("diff-hunk").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("diff-hunk"));
        tag_table.add(&tag);
        tag
    });
    hunk_tag.set_property("foreground", &colors.hunk_fg);
    hunk_tag.set_property("weight-set", true);

    let header_tag = tag_table.lookup("diff-header").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("diff-header"));
        tag_table.add(&tag);
        tag
    });
    header_tag.set_property("foreground", &colors.header_fg);
    header_tag.set_property("weight-set", true);

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
            // For empty added lines, add a space to ensure background colour is rendered
            if rest.is_empty() {
                " ".to_string()
            } else {
                rest.to_string()
            }
        } else if let Some(rest) = line.strip_prefix('-') {
            kind = DiffLineKind::Remove;
            sym = '-';
            o = old_line;
            if let Some(v) = &mut old_line {
                *v += 1;
            }
            // For empty removed lines, add a space to ensure background colour is rendered
            if rest.is_empty() {
                " ".to_string()
            } else {
                rest.to_string()
            }
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

    // Characters per gutter line: " <old> <new> <sym>" (leading space for text padding)
    // old/new are both right-aligned to `width`.
    let gutter_chars = width * 2 + 4; // +4 for: leading space, two separators, symbol

    let mut gutter = String::new();
    for (idx, (o, n, sym)) in gutter_lines.into_iter().enumerate() {
        if idx > 0 {
            gutter.push('\n');
        }
        let o_s = o.map(|v| format!("{:>width$}", v, width = width));
        let n_s = n.map(|v| format!("{:>width$}", v, width = width));

        // Format: " old new sym" (leading space for left padding)
        // Keep a little breathing room between columns for readability.
        gutter.push(' '); // Leading space for text padding
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

/// Apply diff color tags to the gutter buffer based on line kinds.
/// Uses darker/heavier colors than the code view for visual distinction.
fn apply_gutter_line_tags(buffer: &gtk::TextBuffer, kinds: &[DiffLineKind]) {
    let tag_table = buffer.tag_table();
    let colors = get_diff_colors();

    // Create or reuse gutter-specific tags with darker colors
    let add_tag = tag_table.lookup("gutter-add").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("gutter-add"));
        tag_table.add(&tag);
        tag
    });
    add_tag.set_property("paragraph-background", &colors.gutter_add_bg);
    add_tag.set_property("paragraph-background-set", true);

    let remove_tag = tag_table.lookup("gutter-remove").unwrap_or_else(|| {
        let tag = gtk::TextTag::new(Some("gutter-remove"));
        tag_table.add(&tag);
        tag
    });
    remove_tag.set_property("paragraph-background", &colors.gutter_remove_bg);
    remove_tag.set_property("paragraph-background-set", true);

    // Clear any existing tags and apply based on line kind
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
                buffer.apply_tag(&add_tag, &line_start, &line_end);
            }
            DiffLineKind::Remove => {
                buffer.apply_tag(&remove_tag, &line_start, &line_end);
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
    apply_gutter_line_tags(&gutter_buffer, kinds);

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
    gutter_view.set_left_margin(0);
    gutter_view.set_right_margin(0);

    // Size the gutter to the minimum width needed to show line numbers and symbols.
    // (Non-expanding, so the right pane takes the remaining space.)
    // Text padding is included in gutter_chars via leading spaces.
    let probe = "0".repeat(global_gutter_chars.max(1));
    let layout = gutter_view.create_pango_layout(Some(&probe));
    let (probe_px, _) = layout.pixel_size();
    gutter_view.set_width_request(probe_px);
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
    // Allow focus so Ctrl+C works for copying selected text
    view.set_can_focus(true);
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
    view.add_css_class("diff-view");

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
    // No spacing between gutter and code view so colored backgrounds are seamless
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .hexpand(true)
        .build();
    row.append(&gutter_view);
    row.append(&h_scroller);

    row
}

/// Builds the content of a file section: its text diff, its image preview, or
/// the preview above the text diff for images stored as text (e.g. SVG).
fn build_section_child(
    prepared: &PreparedDiffSection,
    global_gutter_chars: usize,
    repo_path: &Path,
) -> gtk::Widget {
    let Some(image) = &prepared.image else {
        return build_file_row(prepared, global_gutter_chars).upcast();
    };
    let has_text = !prepared.kinds.is_empty();
    let comparison = image_diff::build(image, repo_path, &prepared.label, has_text);
    if !has_text {
        return comparison.upcast();
    }
    let column = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    column.append(&comparison);
    column.append(&build_file_row(prepared, global_gutter_chars));
    column.upcast()
}

fn build_file_expander_lazy(
    prepared: &PreparedDiffSection,
    expanded: bool,
    global_gutter_chars: usize,
    repo_path: &Path,
) -> gtk::Expander {
    let expander = gtk::Expander::builder().expanded(expanded).build();

    let filename_row = copy_on_hover::CopyOnHoverRow::new(
        &prepared.label,
        prepared.label.clone(),
        "Copy filename",
        "copy-filename-btn",
        Some(gtk::pango::EllipsizeMode::Middle),
    );
    filename_row.reveal_on_hover(&expander);
    let header = filename_row.with_trailing_spacer();
    header.set_halign(gtk::Align::Start);
    expander.set_label_widget(Some(&header));

    // If expanded initially, build the heavy child once now.
    if expanded {
        let row = build_section_child(prepared, global_gutter_chars, repo_path);
        expander.set_child(Some(&row));
    }

    // Lazy-build / drop the heavy child as the user opens/closes the file section.
    // This keeps the widget tree lightweight during paned/window resizing.
    let prepared_for_cb = prepared.clone();
    let repo_path = repo_path.to_path_buf();

    expander.connect_expanded_notify(move |exp| {
        if exp.is_expanded() {
            if exp.child().is_none() {
                let row = build_section_child(&prepared_for_cb, global_gutter_chars, &repo_path);
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
    rx: mpsc::Receiver<Result<git::CommitDiff, git2::Error>>,
    repo_path: PathBuf,
    diff_files_box: gtk::Box,
    diff_file_tree: DiffFileTree,
    expand_button: gtk::Button,
    collapse_button: gtk::Button,
) {
    match rx.try_recv() {
        Ok(Ok(diff)) => {
            let sections = sections_from_commit_diff(&diff);
            clear_container(&diff_files_box);
            diff_file_tree.set_files(&file_summaries_from_commit_diff(&diff));

            if let Some(preamble) = diff.preamble.as_ref().filter(|s| !s.trim().is_empty()) {
                let preamble_label = gtk::Label::builder()
                    .label(preamble.as_str())
                    .halign(gtk::Align::Start)
                    .wrap(true)
                    .xalign(0.0)
                    .margin_start(10)
                    .margin_end(10)
                    .margin_top(8)
                    .margin_bottom(4)
                    .build();
                preamble_label.add_css_class("dim-label");
                diff_files_box.append(&preamble_label);
            }

            if sections.is_empty() {
                if diff.preamble.is_none() {
                    set_placeholder(
                        &diff_files_box,
                        crate::ui::placeholder::ICON_INFO,
                        "No textual changes to display",
                    );
                }
                update_expand_collapse_buttons(&diff_files_box, &expand_button, &collapse_button);
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
                    image: section.image,
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
                let expander =
                    build_file_expander_lazy(prepared, expanded, global_gutter_chars, &repo_path);
                let diff_files_box_for_notify = diff_files_box.clone();
                let expand_for_notify = expand_button.clone();
                let collapse_for_notify = collapse_button.clone();
                expander.connect_expanded_notify(move |_| {
                    update_expand_collapse_buttons(
                        &diff_files_box_for_notify,
                        &expand_for_notify,
                        &collapse_for_notify,
                    );
                });
                diff_files_box.append(&expander);
            }

            update_expand_collapse_buttons(&diff_files_box, &expand_button, &collapse_button);
        }
        Ok(Err(e)) => {
            diff_file_tree.clear();
            let error_msg = format!("Error loading diff: {}", e);
            set_placeholder(
                &diff_files_box,
                crate::ui::placeholder::ICON_ERROR,
                &error_msg,
            );
            update_expand_collapse_buttons(&diff_files_box, &expand_button, &collapse_button);
        }
        Err(mpsc::TryRecvError::Empty) => {
            let diff_files_box_clone = diff_files_box.clone();
            let diff_file_tree_clone = diff_file_tree.clone();
            let expand_btn_clone = expand_button.clone();
            let collapse_btn_clone = collapse_button.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
                poll_diff_result(
                    rx,
                    repo_path,
                    diff_files_box_clone,
                    diff_file_tree_clone,
                    expand_btn_clone,
                    collapse_btn_clone,
                );
            });
        }
        Err(_) => {
            diff_file_tree.clear();
            set_placeholder(
                &diff_files_box,
                crate::ui::placeholder::ICON_ERROR,
                "Error: channel closed",
            );
            update_expand_collapse_buttons(&diff_files_box, &expand_button, &collapse_button);
        }
    }
}

/// Splits a commit message into its subject (first line) and body (the
/// remaining lines with the leading blank separator removed). The subject is
/// rendered as the header title, so the body must not repeat it.
fn split_commit_message(message: &str) -> (String, String) {
    let mut lines = message.lines();
    let subject = lines.next().unwrap_or("").to_string();
    let body = lines
        .collect::<Vec<_>>()
        .join("\n")
        .trim_start_matches('\n')
        .trim_end()
        .to_string();
    (subject, body)
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
#[allow(clippy::too_many_arguments)]
fn poll_metadata_result(
    rx: mpsc::Receiver<Result<git::CommitMetadata, git2::Error>>,
    title_label: gtk::Label,
    metadata_label: gtk::Label,
    avatar_label: gtk::Label,
    sha_row: gtk::Box,
    sha_label: gtk::Label,
    sha_copy_text: std::rc::Rc<std::cell::RefCell<String>>,
    message_container: gtk::Box,
    commit_message_label: gtk::Label,
    expand_label: gtk::Label,
    full_message: std::rc::Rc<std::cell::RefCell<String>>,
    is_expanded: std::rc::Rc<std::cell::RefCell<bool>>,
) {
    match rx.try_recv() {
        Ok(Ok(metadata)) => {
            clear_metadata_skeleton(
                &title_label,
                &metadata_label,
                &avatar_label,
                &commit_message_label,
            );

            let (title, body) = split_commit_message(&metadata.commit_message);
            title_label.set_text(&title);
            avatar_label.set_text(&author_initials(&metadata.author_name));
            avatar_label.set_visible(true);

            let author = format_author(&metadata.author_name, &metadata.author_email);
            let subtitle = match &metadata.commit_date_time {
                Some(commit_date_time) => format!(
                    "{} \u{00B7} authored {} \u{00B7} committed {}",
                    author, metadata.date_time, commit_date_time
                ),
                None => format!("{} \u{00B7} authored {}", author, metadata.date_time),
            };
            metadata_label.set_text(&subtitle);

            sha_label.set_text(&metadata.git_sha);
            *sha_copy_text.borrow_mut() = metadata.git_sha.clone();
            sha_row.set_visible(true);

            // The subject is already shown as the title, so the body only holds
            // the remaining lines to avoid printing the first line twice. When
            // there is no body, hide the whole container so it contributes no
            // empty space between the header and the footer.
            *full_message.borrow_mut() = body.clone();
            *is_expanded.borrow_mut() = false;

            message_container.set_visible(!body.is_empty());
            let (truncated, has_more) = truncate_to_lines(&body, 5);
            commit_message_label.set_text(&truncated);
            expand_label.set_visible(has_more);
            if has_more {
                expand_label.set_markup("<b>Show more</b>");
            }
        }
        Ok(Err(_)) => {
            clear_metadata_skeleton(
                &title_label,
                &metadata_label,
                &avatar_label,
                &commit_message_label,
            );
        }
        Err(mpsc::TryRecvError::Empty) => {
            let title_label_clone = title_label.clone();
            let metadata_label_clone = metadata_label.clone();
            let avatar_label_clone = avatar_label.clone();
            let sha_row_clone = sha_row.clone();
            let sha_label_clone = sha_label.clone();
            let sha_copy_text_clone = sha_copy_text.clone();
            let message_container_clone = message_container.clone();
            let commit_message_label_clone = commit_message_label.clone();
            let expand_label_clone = expand_label.clone();
            let full_message_clone = full_message.clone();
            let is_expanded_clone = is_expanded.clone();
            glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
                poll_metadata_result(
                    rx,
                    title_label_clone,
                    metadata_label_clone,
                    avatar_label_clone,
                    sha_row_clone,
                    sha_label_clone,
                    sha_copy_text_clone,
                    message_container_clone,
                    commit_message_label_clone,
                    expand_label_clone,
                    full_message_clone,
                    is_expanded_clone,
                );
            });
        }
        Err(_) => {
            clear_metadata_skeleton(
                &title_label,
                &metadata_label,
                &avatar_label,
                &commit_message_label,
            );
        }
    }
}

fn load_range_diff(
    ui: &WindowUi,
    state: &AppState,
    oldest_sha: &str,
    newest_sha: &str,
    count: usize,
) {
    let Some(ref path) = *state.current_path.borrow() else {
        return;
    };

    ui.repo_view.set_diff_chrome_visible(true);
    set_diff_skeleton(&ui.repo_view.diff_files_box);
    ui.repo_view.diff_file_tree.clear();
    set_metadata_skeleton(
        &ui.repo_view.commit_title_label,
        &ui.repo_view.diff_metadata_label,
        &ui.repo_view.diff_avatar_label,
        &ui.repo_view.diff_sha_row,
        &ui.repo_view.commit_message_label,
    );
    ui.repo_view.expand_label.set_visible(false);
    ui.repo_view.reset_commit_message_expansion();
    update_expand_collapse_buttons(
        &ui.repo_view.diff_files_box,
        &ui.repo_view.diff_expand_all_button,
        &ui.repo_view.diff_collapse_all_button,
    );

    let diff_files_box_clone = ui.repo_view.diff_files_box.clone();
    let expand_btn = ui.repo_view.diff_expand_all_button.clone();
    let collapse_btn = ui.repo_view.diff_collapse_all_button.clone();
    let path_clone = path.clone();
    let oldest = oldest_sha.to_string();
    let newest = newest_sha.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let diff_result = git::get_range_diff(path_clone.to_str().unwrap(), &oldest, &newest);
        let _ = tx.send(diff_result);
    });
    poll_diff_result(
        rx,
        path.clone(),
        diff_files_box_clone,
        ui.repo_view.diff_file_tree.clone(),
        expand_btn,
        collapse_btn,
    );

    clear_metadata_skeleton(
        &ui.repo_view.commit_title_label,
        &ui.repo_view.diff_metadata_label,
        &ui.repo_view.diff_avatar_label,
        &ui.repo_view.commit_message_label,
    );
    // A range selection has no single author; hide the avatar and use the title
    // row for the summary.
    ui.repo_view.diff_avatar_label.set_visible(false);
    ui.repo_view
        .commit_title_label
        .set_text(&format!("{count} commits selected"));
    ui.repo_view.diff_metadata_label.set_text("");
    let short_oldest = oldest_sha.get(..7).unwrap_or(oldest_sha);
    let short_newest = newest_sha.get(..7).unwrap_or(newest_sha);
    ui.repo_view
        .diff_sha_label
        .set_text(&format!(" {short_oldest}..{short_newest}"));
    *ui.repo_view.diff_sha_copy_text.borrow_mut() = format!("{oldest_sha}..{newest_sha}");
    ui.repo_view.diff_sha_row.set_visible(true);
    // A range has no commit body; hide the container so it leaves no empty gap.
    ui.repo_view.commit_message_container.set_visible(false);
    ui.repo_view.commit_message_label.set_text("");
    *ui.repo_view.full_message.borrow_mut() = String::new();
}

fn load_commit_diff(ui: &WindowUi, state: &AppState, commit_sha: &str) {
    let Some(ref path) = *state.current_path.borrow() else {
        return;
    };

    ui.repo_view.set_diff_chrome_visible(true);
    // Show skeleton loading state
    set_diff_skeleton(&ui.repo_view.diff_files_box);
    ui.repo_view.diff_file_tree.clear();
    set_metadata_skeleton(
        &ui.repo_view.commit_title_label,
        &ui.repo_view.diff_metadata_label,
        &ui.repo_view.diff_avatar_label,
        &ui.repo_view.diff_sha_row,
        &ui.repo_view.commit_message_label,
    );
    ui.repo_view.expand_label.set_visible(false);
    ui.repo_view.reset_commit_message_expansion();
    update_expand_collapse_buttons(
        &ui.repo_view.diff_files_box,
        &ui.repo_view.diff_expand_all_button,
        &ui.repo_view.diff_collapse_all_button,
    );

    // Load diff in background thread
    let diff_files_box_clone = ui.repo_view.diff_files_box.clone();
    let expand_btn = ui.repo_view.diff_expand_all_button.clone();
    let collapse_btn = ui.repo_view.diff_collapse_all_button.clone();
    let path_clone = path.clone();
    let sha_clone = commit_sha.to_string();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let diff_result = git::get_commit_diff(path_clone.to_str().unwrap(), &sha_clone);
        let _ = tx.send(diff_result);
    });
    poll_diff_result(
        rx,
        path.clone(),
        diff_files_box_clone,
        ui.repo_view.diff_file_tree.clone(),
        expand_btn,
        collapse_btn,
    );

    // Load metadata in background thread
    let title_label_clone = ui.repo_view.commit_title_label.clone();
    let metadata_label_clone = ui.repo_view.diff_metadata_label.clone();
    let avatar_label_clone = ui.repo_view.diff_avatar_label.clone();
    let sha_row_clone = ui.repo_view.diff_sha_row.clone();
    let sha_label_clone = ui.repo_view.diff_sha_label.clone();
    let sha_copy_text_clone = ui.repo_view.diff_sha_copy_text.clone();
    let message_container_clone = ui.repo_view.commit_message_container.clone();
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
        title_label_clone,
        metadata_label_clone,
        avatar_label_clone,
        sha_row_clone,
        sha_label_clone,
        sha_copy_text_clone,
        message_container_clone,
        commit_message_label_clone,
        expand_label_clone,
        full_message_clone,
        is_expanded_clone,
    );
}

pub fn connect(ui: &WindowUi, state: &AppState) {
    // Wire diff header controls
    {
        let diff_files_box = ui.repo_view.diff_files_box.clone();
        let expand_btn = ui.repo_view.diff_expand_all_button.clone();
        let collapse_btn = ui.repo_view.diff_collapse_all_button.clone();
        ui.repo_view
            .diff_expand_all_button
            .connect_clicked(move |_| {
                set_all_file_expanders(&diff_files_box, true);
                update_expand_collapse_buttons(&diff_files_box, &expand_btn, &collapse_btn);
            });
    }
    {
        let diff_files_box = ui.repo_view.diff_files_box.clone();
        let expand_btn = ui.repo_view.diff_expand_all_button.clone();
        let collapse_btn = ui.repo_view.diff_collapse_all_button.clone();
        ui.repo_view
            .diff_collapse_all_button
            .connect_clicked(move |_| {
                set_all_file_expanders(&diff_files_box, false);
                update_expand_collapse_buttons(&diff_files_box, &expand_btn, &collapse_btn);
            });
    }

    // Clicking a file in the changed-files column jumps to its diff.
    {
        let diff_files_box = ui.repo_view.diff_files_box.clone();
        let diff_scrolled_window = ui.repo_view.diff_scrolled_window.clone();
        ui.repo_view.diff_file_tree.on_file_activated(move |index| {
            scroll_to_file(&diff_files_box, &diff_scrolled_window, index);
        });
    }

    // Changed-files chips and image outlines share the diff colors; installed
    // app-wide so they can be reloaded on theme changes like the diff text tags.
    let diff_colors_provider = gtk::CssProvider::new();
    diff_colors_provider.load_from_string(&diff_colors_css(get_diff_colors()));
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &diff_colors_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }

    // Refresh diff colors when theme changes (light/dark mode switch)
    let diff_files_box_for_theme = ui.repo_view.diff_files_box.clone();
    adw::StyleManager::default().connect_dark_notify(move |_| {
        refresh_diff_colors(&diff_files_box_for_theme);
        diff_colors_provider.load_from_string(&diff_colors_css(get_diff_colors()));
    });

    let ui_for_selection = ui.clone();
    let state_for_selection = state.clone();

    ui.repo_view
        .commit_list
        .selection_model
        .connect_selection_changed(move |_position, _n_items, _model| {
            let commit_list = &ui_for_selection.repo_view.commit_list;
            let indices = commit_list.selected_indices();
            if indices.is_empty() {
                // No path: leave the current diff in place for the close crossfade.
                // Empty list: first page is still loading — don't flash a message.
                if state_for_selection.current_path.borrow().is_some() {
                    if commit_list.store.n_items() == 0 {
                        ui_for_selection.repo_view.reset_diff(None);
                    } else {
                        ui_for_selection.repo_view.reset_diff(Some(
                            "Select a commit (or a range) to view the commit diff",
                        ));
                    }
                }
            } else if indices.len() == 1 {
                if let Some(sha) = commit_list.selected_commit_sha() {
                    load_commit_diff(&ui_for_selection, &state_for_selection, &sha);
                }
            } else if let Some((oldest, newest, count)) = commit_list.selected_commit_range() {
                load_range_diff(
                    &ui_for_selection,
                    &state_for_selection,
                    &oldest,
                    &newest,
                    count,
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{author_initials, format_author, sections_from_commit_diff, split_commit_message};
    use crate::git::{CommitDiff, FileChange, FileChangeKind};
    use crate::ui::image_diff::ImageDiffSource;
    use git2::{FileMode, Oid};

    #[test]
    fn split_commit_message_separates_subject_and_body() {
        let (subject, body) = split_commit_message("Fix the bug\n\nDetails line 1\nDetails line 2");
        assert_eq!(subject, "Fix the bug");
        assert_eq!(body, "Details line 1\nDetails line 2");
    }

    #[test]
    fn split_commit_message_subject_only_has_empty_body() {
        let (subject, body) = split_commit_message("Just a subject\n");
        assert_eq!(subject, "Just a subject");
        assert_eq!(body, "");
    }

    #[test]
    fn split_commit_message_trims_extra_blank_lines() {
        let (subject, body) = split_commit_message("Subject\n\n\nBody\n\n");
        assert_eq!(subject, "Subject");
        assert_eq!(body, "Body");
    }

    #[test]
    fn format_author_appends_email() {
        assert_eq!(
            format_author("Mark Deepwell", "mark@example.com"),
            "Mark Deepwell <mark@example.com>"
        );
    }

    #[test]
    fn format_author_omits_empty_email() {
        assert_eq!(format_author("Mark Deepwell", ""), "Mark Deepwell");
    }

    #[test]
    fn author_initials_uses_first_two_word_initials() {
        assert_eq!(author_initials("Mark Deepwell"), "MD");
        assert_eq!(author_initials("ada b. lovelace"), "AB");
    }

    #[test]
    fn author_initials_uses_two_chars_for_single_word() {
        assert_eq!(author_initials("mark"), "MA");
        assert_eq!(author_initials("x"), "X");
    }

    #[test]
    fn author_initials_handles_empty_name() {
        assert_eq!(author_initials(""), "");
        assert_eq!(author_initials("   "), "");
    }

    fn oid(n: u8) -> Oid {
        Oid::from_bytes(&[n; 20]).unwrap()
    }

    fn change(
        kind: FileChangeKind,
        path: &str,
        old_oid: Option<Oid>,
        new_oid: Option<Oid>,
        binary: bool,
        patch: &str,
    ) -> FileChange {
        FileChange {
            kind,
            old_path: Some(path.to_string()),
            new_path: Some(path.to_string()),
            old_oid,
            new_oid,
            old_mode: FileMode::Blob,
            new_mode: FileMode::Blob,
            binary,
            patch: patch.to_string(),
        }
    }

    fn sections(files: Vec<FileChange>) -> Vec<(String, Option<ImageDiffSource>)> {
        let diff = CommitDiff {
            preamble: None,
            files,
        };
        sections_from_commit_diff(&diff)
            .into_iter()
            .map(|section| (section.text, section.image))
            .collect()
    }

    const BINARY_PATCH: &str = "Binary files a/x differ\n";

    #[test]
    fn binary_image_is_previewed_without_text() {
        let file = change(
            FileChangeKind::Modified,
            "logo.png",
            Some(oid(1)),
            Some(oid(2)),
            true,
            BINARY_PATCH,
        );
        let expected = ImageDiffSource {
            old: Some(oid(1)),
            new: Some(oid(2)),
        };
        assert_eq!(sections(vec![file]), vec![(String::new(), Some(expected))]);
    }

    #[test]
    fn text_image_is_previewed_above_its_text_diff() {
        let patch = "@@ -1 +1 @@\n-<svg/>\n+<svg width=\"2\"/>\n";
        let file = change(
            FileChangeKind::Modified,
            "icon.svg",
            Some(oid(1)),
            Some(oid(2)),
            false,
            patch,
        );
        let [(text, image)] = sections(vec![file]).try_into().unwrap();
        assert_eq!(text, patch);
        assert!(image.is_some());
    }

    #[test]
    fn non_image_binary_keeps_binary_notice() {
        let file = change(
            FileChangeKind::Modified,
            "archive.zip",
            Some(oid(1)),
            Some(oid(2)),
            true,
            BINARY_PATCH,
        );
        assert_eq!(sections(vec![file]), vec![(BINARY_PATCH.to_string(), None)]);
    }

    #[test]
    fn deleted_image_previews_old_side_only() {
        let file = change(
            FileChangeKind::Deleted,
            "logo.png",
            Some(oid(1)),
            None,
            true,
            BINARY_PATCH,
        );
        let [(_, image)] = sections(vec![file]).try_into().unwrap();
        assert_eq!(
            image,
            Some(ImageDiffSource {
                old: Some(oid(1)),
                new: None,
            })
        );
    }

    #[test]
    fn mode_only_image_change_is_described_instead_of_previewed() {
        let mut file = change(
            FileChangeKind::Modified,
            "logo.png",
            Some(oid(1)),
            Some(oid(1)),
            true,
            BINARY_PATCH,
        );
        file.new_mode = FileMode::BlobExecutable;
        assert_eq!(
            sections(vec![file]),
            vec![(
                "File mode changed from 100644 to 100755 with no content changes\n".to_string(),
                None
            )]
        );
    }

    #[test]
    fn image_without_blob_is_not_previewed() {
        // e.g. a submodule or symlink named like an image.
        let file = change(
            FileChangeKind::Typechange,
            "logo.png",
            None,
            None,
            false,
            "",
        );
        let [(_, image)] = sections(vec![file]).try_into().unwrap();
        assert_eq!(image, None);
    }
}
