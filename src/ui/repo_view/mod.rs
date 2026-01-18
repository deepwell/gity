use gtk::{
    glib::{self},
    prelude::*,
};

use std::cell::RefCell;
use std::rc::Rc;

use crate::ui::{BranchPanel, CommitList, CommitPagingState};

#[derive(Clone)]
pub struct RepoView {
    /// Root widget containing the entire repository view (search + panels + diff).
    pub widget: gtk::Box,

    // Search UI
    pub search_bar: gtk::SearchBar,
    pub search_entry: gtk::Entry,
    pub search_spinner: gtk::Spinner,
    pub search_status_label: gtk::Label,
    pub last_search_status: Rc<RefCell<String>>,

    // Panels
    pub branch_panel: BranchPanel,
    pub commit_list: CommitList,
    pub commit_paging_state: Rc<RefCell<CommitPagingState>>,

    // Diff UI
    pub diff_files_box: gtk::Box,
    pub diff_label: gtk::Label,
    pub diff_expand_toggle_button: gtk::Button,
    pub commit_message_label: gtk::Label,
    pub expand_label: gtk::Label,
    pub full_message: Rc<RefCell<String>>,
    pub is_expanded: Rc<RefCell<bool>>,

    // Layout widgets (persistence reads/writes these positions)
    pub main_content_paned: gtk::Paned,
    pub horizontal_paned: gtk::Paned,
}

impl RepoView {
    /// Builds the full repo screen (search bar + branch/commit panels + diff view).
    ///
    /// The `window` is only used for a couple of UX touches (entry width sizing).
    pub fn new(window: &gtk::ApplicationWindow) -> Self {
        // Search bar component
        let search_entry = gtk::Entry::builder().placeholder_text("Search...").build();
        let search_spinner = gtk::Spinner::builder()
            .spinning(false)
            .visible(false)
            .build();
        search_spinner.set_valign(gtk::Align::Center);
        search_spinner.set_halign(gtk::Align::End);
        search_spinner.set_margin_end(8);
        search_spinner.set_can_target(false);

        // Add some padding so the entry text doesn't overlap the spinner.
        // Note: this is best-effort; GTK theme nodes may vary slightly.
        search_entry.add_css_class("search-entry-with-spinner");

        // Fixed width so showing/hiding results doesn't shift the centered search entry.
        let search_status_label = gtk::Label::builder().label("").margin_start(6).build();
        search_status_label.set_width_chars(20);
        search_status_label.set_xalign(0.0);

        let last_search_status: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));

        let search_container = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::Center)
            .build();

        search_entry.set_hexpand(false);
        search_entry.set_halign(gtk::Align::Center);

        // Keep entry width at 60% of window width
        let search_entry_for_update = search_entry.clone();
        let update_entry_width = move |win: &gtk::ApplicationWindow| {
            let (width, _) = win.default_size();
            let target_width = (width as f64 * 0.6) as i32;
            search_entry_for_update.set_width_request(target_width);
        };

        update_entry_width(window);
        let search_entry_for_resize = search_entry.clone();
        window.connect_default_width_notify(move |win| {
            let (width, _) = win.default_size();
            let target_width = (width as f64 * 0.6) as i32;
            search_entry_for_resize.set_width_request(target_width);
        });

        // Put the spinner *inside* the entry area by overlaying it.
        let search_entry_overlay = gtk::Overlay::new();
        search_entry_overlay.set_child(Some(&search_entry));
        search_entry_overlay.add_overlay(&search_spinner);
        search_container.append(&search_entry_overlay);
        search_container.append(&search_status_label);

        let search_bar = gtk::SearchBar::builder().search_mode_enabled(false).build();
        search_bar.set_child(Some(&search_container));

        // ESC closes search bar while entry has focus
        let search_bar_for_esc = search_bar.clone();
        let search_key_controller = gtk::EventControllerKey::new();
        search_key_controller.connect_key_pressed(move |_, keyval, _, _| {
            if keyval == gtk::gdk::Key::Escape {
                search_bar_for_esc.set_search_mode(false);
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        search_entry.add_controller(search_key_controller);

        // Panels
        let commit_list = CommitList::new();
        let commit_paging_state = commit_list.paging_state();

        let branch_panel = BranchPanel::new(&[]);
        let side_panel = branch_panel.widget.clone();

        // Diff UI
        let diff_files_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .vexpand(true)
            .build();

        // Initial empty state
        diff_files_box.append(
            &gtk::Label::builder()
                .label("Select a commit to view diff")
                .halign(gtk::Align::Start)
                .build(),
        );

        let diff_scrolled_window = gtk::ScrolledWindow::builder()
            .min_content_height(200)
            .build();
        diff_scrolled_window.set_child(Some(&diff_files_box));
        diff_scrolled_window.set_vexpand(true);
        diff_scrolled_window.set_hexpand(true);

        let diff_label = gtk::Label::builder()
            .label("Commit Diff")
            .halign(gtk::Align::Start)
            .selectable(true)
            .build();
        diff_label.set_hexpand(true);
        diff_label.set_xalign(0.0);

        // Diff header controls
        let diff_expand_toggle_button = gtk::Button::builder()
            .label("Expand all")
            .tooltip_text("Expand/collapse all file diffs")
            .build();
        diff_expand_toggle_button.add_css_class("flat");
        diff_expand_toggle_button.set_sensitive(false);

        let diff_header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .margin_start(10)
            .margin_end(10)
            .margin_top(10)
            .margin_bottom(5)
            .spacing(8)
            .build();
        diff_header.append(&diff_label);
        diff_header.append(&diff_expand_toggle_button);

        let commit_message_container = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_start(10)
            .margin_bottom(5)
            .build();

        let commit_message_label = gtk::Label::builder()
            .label("")
            .halign(gtk::Align::Start)
            .wrap(true)
            .selectable(true)
            .build();

        let expand_label = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .margin_top(5)
            .visible(false)
            .use_markup(true)
            .build();
        expand_label.set_markup("<b>Show more</b>");
        expand_label.set_cursor_from_name(Some("pointer"));

        commit_message_container.append(&commit_message_label);
        commit_message_container.append(&expand_label);

        let full_message = Rc::new(RefCell::new(String::new()));
        let is_expanded = Rc::new(RefCell::new(false));

        let gesture = gtk::GestureClick::new();
        gesture.set_button(1);

        let commit_message_label_for_toggle = commit_message_label.clone();
        let expand_label_for_toggle = expand_label.clone();
        let full_message_for_toggle = full_message.clone();
        let is_expanded_for_toggle = is_expanded.clone();
        gesture.connect_pressed(move |_, _, _, _| {
            let mut expanded = is_expanded_for_toggle.borrow_mut();
            *expanded = !*expanded;

            let full_msg = full_message_for_toggle.borrow();
            if *expanded {
                commit_message_label_for_toggle.set_text(&full_msg);
                expand_label_for_toggle.set_markup("<b>Show less</b>");
            } else {
                let lines: Vec<&str> = full_msg.lines().collect();
                let has_more = lines.len() > 5;
                let truncated = if has_more {
                    lines[..5].join("\n")
                } else {
                    full_msg.clone()
                };
                commit_message_label_for_toggle.set_text(&truncated);
                expand_label_for_toggle.set_visible(has_more);
                if has_more {
                    expand_label_for_toggle.set_markup("<b>Show more</b>");
                }
            }
        });
        expand_label.add_controller(gesture);

        let diff_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        diff_box.append(&diff_header);
        diff_box.append(&commit_message_container);
        diff_box.append(&diff_scrolled_window);

        // Layout (paned widgets)
        let main_content_paned = gtk::Paned::new(gtk::Orientation::Vertical);
        main_content_paned.set_start_child(Some(&commit_list.widget));
        main_content_paned.set_end_child(Some(&diff_box));
        main_content_paned.set_resize_start_child(true);
        main_content_paned.set_resize_end_child(true);
        main_content_paned.set_shrink_start_child(true);
        main_content_paned.set_shrink_end_child(true);

        let horizontal_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        horizontal_paned.set_start_child(Some(&side_panel));
        horizontal_paned.set_end_child(Some(&main_content_paned));
        horizontal_paned.set_resize_start_child(false);
        horizontal_paned.set_resize_end_child(true);
        horizontal_paned.set_shrink_start_child(true);
        horizontal_paned.set_shrink_end_child(true);

        // Main view: search bar + paned layout
        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        widget.append(&search_bar);
        widget.append(&horizontal_paned);

        Self {
            widget,
            search_bar,
            search_entry,
            search_spinner,
            search_status_label,
            last_search_status,
            branch_panel,
            commit_list,
            commit_paging_state,
            diff_files_box,
            diff_label,
            diff_expand_toggle_button,
            commit_message_label,
            expand_label,
            full_message,
            is_expanded,
            main_content_paned,
            horizontal_paned,
        }
    }
}
