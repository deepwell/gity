/// Application-wide CSS tweaks.
///
/// This is installed once at startup (or when the main window is built).
pub fn install() {
    // Best-effort: if there's no default display (headless/tests), just skip.
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };

    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
.search-entry-with-spinner { padding-right: 28px; }

/* Diff gutter (left bar with line numbers and +/-) – theme-aware for light/dark */
textview.diff-gutter,
textview.diff-gutter text {
  background-color: @view_bg_color;
  color: @view_fg_color;
}

/* Diff view (main diff content) – theme-aware for light/dark */
textview.diff-view,
textview.diff-view text {
  background-color: @view_bg_color;
  color: @view_fg_color;
}

/* Recent repository cards */
.repo-card {
  background-color: alpha(@card_bg_color, 0.8);
  border-radius: 12px;
  border: 1px solid alpha(@borders, 0.5);
  transition: all 150ms ease-in-out;
}

.repo-card:hover {
  background-color: alpha(@accent_bg_color, 0.15);
  border-color: @accent_color;
  box-shadow: 0 2px 8px alpha(black, 0.1);
}
"#,
    );

    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
