//! Shared centered placeholder widget for empty/error states.
//!
//! Provides an `AdwStatusPage`-style placeholder (a large dimmed symbolic icon
//! above dimmed text) used to fill otherwise-empty areas such as the diff pane
//! when no commit is selected, a commit has no textual changes, or an error
//! occurred. This keeps those states visually consistent with the welcome
//! screen rather than showing a bare left-aligned label.

use gtk::prelude::*;

/// Icon shown for informational placeholders (no commit selected, no changes).
pub const ICON_INFO: &str = "text-x-generic-symbolic";
/// Icon shown for error placeholders.
pub const ICON_ERROR: &str = "dialog-error-symbolic";

/// Build a centered placeholder with a dimmed icon above dimmed text.
///
/// The returned widget expands to fill its parent so the content is centered
/// both horizontally and vertically.
pub fn centered(icon_name: &str, text: &str) -> gtk::Widget {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .vexpand(true)
        .spacing(12)
        .build();

    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_pixel_size(48);
    icon.add_css_class("dim-label");
    container.append(&icon);

    let label = gtk::Label::builder()
        .label(text)
        .justify(gtk::Justification::Center)
        .wrap(true)
        .build();
    label.add_css_class("title-4");
    label.add_css_class("dim-label");
    container.append(&label);

    container.upcast()
}
