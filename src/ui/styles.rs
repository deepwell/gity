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

/* Diff gutter (left bar with line numbers and +/-) */
textview.diff-gutter,
textview.diff-gutter text {
  background-color:rgb(250, 250, 250);
  color: #2b2b2b;
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
