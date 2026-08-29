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

/* Highlight the welcome screen while a folder is dragged over it */
.welcome-drop-target:drop(active) {
  background-color: alpha(@accent_bg_color, 0.08);
}

/* Circular author avatar in the commit metadata header */
.commit-avatar {
  min-width: 36px;
  min-height: 36px;
  border-radius: 9999px;
  background-color: alpha(@accent_bg_color, 0.25);
  color: @accent_color;
  font-weight: bold;
}

/* Bold commit subject (first line of the message) in the header */
.commit-title {
  font-weight: bold;
  font-size: 1.1em;
}

/* Expand/collapse controls float over the metadata text, so give the wrapper
   the panel's background color to occlude any text behind them. */
.commit-controls {
  background-color: @window_bg_color;
  border-radius: 6px;
}

.commit-control-button:hover {
  background-color: shade(@window_bg_color, 0.92);
}
"#,
    );

    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
}
