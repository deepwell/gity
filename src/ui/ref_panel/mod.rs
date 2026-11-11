//! Ref panel UI component for displaying and selecting git refs.
//!
//! This module provides the `RefPanel` widget which displays a list of
//! git branches, tags, stashes, and remote-tracking branches with their last commit time and
//! allows single-selection. All sections are collapsible with state persisted
//! to gsettings. A filter bar (hidden until requested) narrows every section
//! to the refs matching the typed text.
//!
//! The panel is one `gtk::ListView` over a `gtk::TreeListModel`: each section,
//! and each remote within Remotes, is an expandable header row whose children
//! are its refs. Only rows scrolled into view get widgets, so a repository
//! with hundreds of tags opens as quickly as one with a handful.

use chrono::{DateTime, Utc};
use gtk::{gio, glib, prelude::*};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::{Rc, Weak};

use crate::APP_ID;
use crate::git::{BranchInfo, StashInfo, TagInfo};

/// Type of git reference (branch, remote branch, tag, or stash).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefType {
    Branch,
    Remote,
    Tag,
    Stash,
}

/// A create or delete request raised from the panel's header buttons and
/// row context menus (see `on_ref_action`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefAction {
    NewBranch,
    NewTag,
    Delete { name: String, ref_type: RefType },
}

/// Information about the currently selected reference.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SelectedRef {
    name: String,
    ref_type: RefType,
}

/// A top-level section of the panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Branches,
    Tags,
    Stashes,
    Remotes,
}

impl Section {
    /// Every section, in display order.
    const ALL: [Section; 4] = [
        Section::Branches,
        Section::Tags,
        Section::Stashes,
        Section::Remotes,
    ];

    fn title(self) -> &'static str {
        match self {
            Section::Branches => "Branches",
            Section::Tags => "Tags",
            Section::Stashes => "Stashes",
            Section::Remotes => "Remotes",
        }
    }

    /// The gsettings key that persists whether the section is expanded.
    fn settings_key(self) -> &'static str {
        match self {
            Section::Branches => "branches-expanded",
            Section::Tags => "tags-expanded",
            Section::Stashes => "stashes-expanded",
            Section::Remotes => "remotes-expanded",
        }
    }

    /// Singular and plural nouns for the header's count tooltip.
    fn count_nouns(self) -> (&'static str, &'static str) {
        match self {
            Section::Branches => ("branch", "branches"),
            Section::Tags => ("tag", "tags"),
            Section::Stashes => ("stash", "stashes"),
            Section::Remotes => ("remote", "remotes"),
        }
    }

    /// Tooltip and action of the header's "+" button, if it has one.
    fn create_button(self) -> Option<(&'static str, &'static str)> {
        match self {
            Section::Branches => Some(("New Branch", "refs.new-branch")),
            Section::Tags => Some(("New Tag", "refs.new-tag")),
            Section::Stashes | Section::Remotes => None,
        }
    }
}

/// What a header row heads: a top-level section or one remote in Remotes.
#[derive(Debug, Clone, PartialEq, Eq)]
enum HeaderKind {
    Section(Section),
    Remote(String),
}

/// Everything a ref row displays.
#[derive(Debug, Clone, PartialEq, Eq)]
struct RefEntry {
    info: SelectedRef,
    /// Row text: the ref name, a remote branch without its remote, or a
    /// stash's message.
    label: String,
    tooltip: String,
    time_ago: String,
    /// Shows the checkmark (the checked-out local branch).
    checked_out: bool,
    /// The text the filter matches against.
    filter_text: String,
}

/// An item of the panel's tree model.
enum PanelItem {
    /// An expandable header. `children` are its rows with the panel filter
    /// applied; the unfiltered rows are `children.model()`.
    Header {
        kind: HeaderKind,
        children: gtk::FilterListModel,
    },
    Ref(RefEntry),
}

/// Indent of a remote's header and branches inside the Remotes section.
const REMOTE_INDENT: i32 = 8;
/// Widget names that tell the two kinds of row widget apart on recycling.
const HEADER_ROW: &str = "header-row";
const REF_ROW: &str = "ref-row";

/// One section's rows: all of them, and those passing the filter.
struct SectionModel {
    section: Section,
    /// Ref items, or (for Remotes) one header item per remote.
    store: gio::ListStore,
    filtered: gtk::FilterListModel,
}

/// A panel widget that displays local branches, tags, and remote-tracking branches.
///
/// The panel shows refs in separate collapsible sections:
/// - **Branches** — local branches with a checkmark on the checked-out branch
/// - **Tags**
/// - **Stashes** — hidden when the repository has no stashes
/// - **Remotes** — grouped by remote name (e.g. `origin`), each collapsible
///
/// Branches are sorted with "main" or "master" first, then by latest commit time.
/// Tags are sorted alphabetically using natural sort order.
///
/// Expanded/collapsed state is persisted to gsettings.
///
/// A filter bar above the sections (see `show_filter`) hides the refs that do
/// not match the typed text.
#[derive(Clone)]
pub struct RefPanel {
    /// The root widget container
    pub widget: gtk::Box,
    inner: Rc<Inner>,
}

struct Inner {
    /// The root widget container (also hosts the context menu popover)
    widget: gtk::Box,
    /// The search bar holding the filter entry (hidden until `show_filter`)
    filter_bar: gtk::SearchBar,
    /// The entry whose text filters the ref rows
    filter_entry: gtk::SearchEntry,
    /// Lowercased filter text, shared with the model filters
    filter_query: Rc<RefCell<String>>,
    /// Shown instead of the sections when the filter matches no refs
    no_matches_label: gtk::Label,
    list_view: gtk::ListView,
    selection: gtk::SingleSelection,
    /// Headers and the rows of expanded headers, flattened for the list view
    tree: gtk::TreeListModel,
    /// Hides empty sections (always Stashes; any of them while filtering)
    section_filter: gtk::CustomFilter,
    /// Hides remotes without matching branches while filtering
    remote_filter: gtk::CustomFilter,
    /// Hides refs that don't match the filter text
    ref_filter: gtk::CustomFilter,
    /// Per-section models, in display order
    sections: Vec<SectionModel>,
    /// Handler invoked when a ref row is activated (set via `on_ref_selected`)
    activate_handler: RefCell<Option<Rc<dyn Fn(&str, RefType)>>>,
    /// Handler invoked for create/delete requests (set via `on_ref_action`)
    ref_action_handler: Rc<RefCell<Option<Rc<dyn Fn(RefAction)>>>>,
    /// Name of the checked-out branch, which can't be deleted
    checked_out_branch: RefCell<Option<String>>,
    /// The selected ref. It stays selected while its row is hidden (in a
    /// collapsed section or filtered out) and is reselected when it shows.
    selected_ref: RefCell<Option<SelectedRef>>,
    /// The ref whose context menu is open
    context_ref: Rc<RefCell<Option<SelectedRef>>>,
    delete_action: gio::SimpleAction,
    context_popover: gtk::PopoverMenu,
    ref_menu: gio::Menu,
    stash_menu: gio::Menu,
    /// GSettings for persisting expanded state
    settings: gio::Settings,
    /// Set while the models are being changed in several steps; the
    /// selection is synced once at the end (see `batch`).
    batching: Cell<bool>,
}

impl RefPanel {
    /// Create a new RefPanel with the given branches.
    ///
    /// # Arguments
    /// * `branches` - Slice of branch information to display
    pub fn new(branches: &[BranchInfo]) -> Self {
        let inner = Rc::new_cyclic(|weak| Inner::new(weak.clone()));
        inner.connect_signals();
        let panel = Self {
            widget: inner.widget.clone(),
            inner,
        };
        panel.update_refs(branches, &[], &[], None, None);
        panel
    }

    /// Update the panel with new lists of branches, remote branches, and stashes.
    /// Tags are listed separately by `set_tags`, since they load in the
    /// background; the current tags stay listed until then.
    ///
    /// Attempts to preserve the current selection if possible.
    pub fn update_refs(
        &self,
        branches: &[BranchInfo],
        remote_branches: &[BranchInfo],
        stashes: &[StashInfo],
        checked_out_branch: Option<&str>,
        current_ref_name: Option<&str>,
    ) {
        let inner = &self.inner;
        let preserved = inner.selected_ref.borrow().clone();

        inner.batch(|| {
            *inner.checked_out_branch.borrow_mut() = checked_out_branch.map(String::from);

            replace_items(
                &inner.section(Section::Branches).store,
                sort_branches(branches)
                    .iter()
                    .map(|b| PanelItem::Ref(branch_entry(b, checked_out_branch))),
            );
            replace_items(
                &inner.section(Section::Stashes).store,
                stashes.iter().map(|s| PanelItem::Ref(stash_entry(s))),
            );
            replace_items(
                &inner.section(Section::Remotes).store,
                group_remote_branches(remote_branches)
                    .into_iter()
                    .map(|(remote, branches)| {
                        let store = gio::ListStore::new::<glib::BoxedAnyObject>();
                        replace_items(
                            &store,
                            branches.iter().map(|b| PanelItem::Ref(remote_entry(b))),
                        );
                        PanelItem::Header {
                            kind: HeaderKind::Remote(remote),
                            children: gtk::FilterListModel::new(
                                Some(store),
                                Some(inner.ref_filter.clone()),
                            ),
                        }
                    }),
            );

            if let Some(ref_info) = preserved {
                inner.select_ref(&ref_info.name);
            } else if let Some(name) = current_ref_name {
                inner.select_ref(name);
            } else if let Some(name) = checked_out_branch {
                inner.select_ref(name);
            }
            inner.ensure_default_selection();

            // The rows changed, so section visibility must be recomputed.
            inner.apply_filter();
        });
    }

    /// Replace the listed tags, keeping the selection. If the selected ref was
    /// a tag that is gone, the default ref is selected instead.
    pub fn set_tags(&self, tags: &[TagInfo]) {
        let inner = &self.inner;
        inner.batch(|| {
            replace_items(
                &inner.section(Section::Tags).store,
                sort_tags(tags).iter().map(|t| PanelItem::Ref(tag_entry(t))),
            );
            inner.ensure_default_selection();
            // The Tags section's visibility under the filter may have changed.
            inner.apply_filter();
        });
    }

    /// Reveal the filter bar and focus its entry.
    pub fn show_filter(&self) {
        self.inner.filter_bar.set_search_mode(true);
        self.inner.filter_entry.grab_focus();
    }

    /// Hide the filter bar and list every ref again.
    pub fn hide_filter(&self) {
        self.inner.filter_bar.set_search_mode(false);
    }

    /// Whether keyboard focus is in the filter entry.
    pub fn filter_has_focus(&self) -> bool {
        self.inner
            .filter_entry
            .state_flags()
            .contains(gtk::StateFlags::FOCUS_WITHIN)
    }

    /// Select a ref by name.
    ///
    /// # Arguments
    /// * `ref_name` - Name of the branch, remote branch, tag, or stash to select
    ///
    /// # Returns
    /// `true` if the ref was found and selected, `false` otherwise.
    pub fn select_ref(&self, ref_name: &str) -> bool {
        self.inner.select_ref(ref_name)
    }

    /// Register a callback for when a ref is selected (activated).
    ///
    /// # Arguments
    /// * `callback` - Function called with the ref name and type when activated
    pub fn on_ref_selected<F: Fn(&str, RefType) + Clone + 'static>(&self, callback: F) {
        *self.inner.activate_handler.borrow_mut() = Some(Rc::new(callback));
    }

    /// Register a callback for create and delete requests from the header
    /// buttons and row context menus.
    pub fn on_ref_action<F: Fn(RefAction) + 'static>(&self, callback: F) {
        *self.inner.ref_action_handler.borrow_mut() = Some(Rc::new(callback));
    }
}

impl Inner {
    fn new(weak: Weak<Inner>) -> Self {
        let settings = gio::Settings::new(APP_ID);

        let side_panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["ref-panel"])
            .build();
        side_panel.set_width_request(150);

        // Filter bar, revealed by `show_filter`. Connecting the entry lets the
        // bar close itself on Escape and clear the text when it closes.
        let filter_entry = gtk::SearchEntry::builder()
            .placeholder_text("Filter by name")
            .hexpand(true)
            .build();
        let filter_bar = gtk::SearchBar::builder().child(&filter_entry).build();
        filter_bar.connect_entry(&filter_entry);
        side_panel.append(&filter_bar);
        let filter_query = Rc::new(RefCell::new(String::new()));

        let no_matches_label = gtk::Label::builder()
            .label("No matching branches, tags, or stashes")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .margin_top(24)
            .margin_start(12)
            .margin_end(12)
            .visible(false)
            .build();
        no_matches_label.add_css_class("dim-label");
        side_panel.append(&no_matches_label);

        // Filters, innermost first: a header's visibility depends on how many
        // of its rows pass the filters below it.
        let ref_filter = {
            let query = filter_query.clone();
            gtk::CustomFilter::new(move |obj| {
                let query = query.borrow();
                query.is_empty()
                    || with_item(obj, |item| {
                        matches!(item, PanelItem::Ref(entry) if filter_matches(&entry.filter_text, &query))
                    })
                    .unwrap_or(false)
            })
        };
        let remote_filter = {
            let query = filter_query.clone();
            gtk::CustomFilter::new(move |obj| {
                query.borrow().is_empty()
                    || header_children(obj).is_some_and(|children| children.n_items() > 0)
            })
        };
        let section_filter = {
            let query = filter_query.clone();
            gtk::CustomFilter::new(move |obj| {
                let filtering = !query.borrow().is_empty();
                with_item(obj, |item| match item {
                    PanelItem::Header { kind, children } => {
                        let matched = children.n_items() > 0;
                        // Stashes is hidden whenever it is empty, the other
                        // sections only while filtering.
                        matched || !(filtering || *kind == HeaderKind::Section(Section::Stashes))
                    }
                    PanelItem::Ref(_) => true,
                })
                .unwrap_or(true)
            })
        };

        let sections: Vec<SectionModel> = Section::ALL
            .into_iter()
            .map(|section| {
                let store = gio::ListStore::new::<glib::BoxedAnyObject>();
                let filter = if section == Section::Remotes {
                    &remote_filter
                } else {
                    &ref_filter
                };
                let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter.clone()));
                SectionModel {
                    section,
                    store,
                    filtered,
                }
            })
            .collect();

        let section_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        for model in &sections {
            section_store.append(&glib::BoxedAnyObject::new(PanelItem::Header {
                kind: HeaderKind::Section(model.section),
                children: model.filtered.clone(),
            }));
        }
        let visible_sections =
            gtk::FilterListModel::new(Some(section_store), Some(section_filter.clone()));

        // New rows start expanded, so a remote that appears on refresh is
        // open; `apply_filter` sets each section's saved state.
        let tree = gtk::TreeListModel::new(visible_sections, false, true, |obj| {
            header_children(obj).map(|children| children.upcast())
        });
        let selection = gtk::SingleSelection::new(Some(tree.clone()));
        selection.set_autoselect(false);
        selection.set_can_unselect(true);

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_bind(move |_, obj| {
            if let (Some(panel), Some(list_item)) = (weak.upgrade(), obj.downcast_ref()) {
                panel.bind_list_item(list_item);
            }
        });
        factory.connect_unbind(|_, obj| {
            let header = obj
                .downcast_ref::<gtk::ListItem>()
                .and_then(|list_item| list_item.child())
                .filter(|child| child.widget_name() == HEADER_ROW);
            if let Some(expander) = header
                .and_then(|header| header.first_child())
                .and_downcast::<gtk::TreeExpander>()
            {
                expander.set_list_row(None);
            }
        });

        let list_view = gtk::ListView::new(Some(selection.clone()), Some(factory));
        list_view.add_css_class("ref-panel-list");
        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&list_view)
            .build();
        side_panel.append(&scrolled);
        side_panel.set_vexpand(true);

        // Create/delete actions, shared by the header buttons and the row
        // context menus. Remote branches have no menu.
        let ref_action_handler: Rc<RefCell<Option<Rc<dyn Fn(RefAction)>>>> =
            Rc::new(RefCell::new(None));
        let context_ref = Rc::new(RefCell::new(None));
        let delete_action = install_ref_actions(&side_panel, &ref_action_handler, &context_ref);
        let ref_menu = gio::Menu::new();
        let create_section = gio::Menu::new();
        create_section.append(Some("New Branch…"), Some("refs.new-branch"));
        create_section.append(Some("New Tag…"), Some("refs.new-tag"));
        ref_menu.append_section(None, &create_section);
        let delete_section = gio::Menu::new();
        delete_section.append(Some("Delete…"), Some("refs.delete"));
        ref_menu.append_section(None, &delete_section);
        let stash_menu = gio::Menu::new();
        stash_menu.append(Some("Delete…"), Some("refs.delete"));
        // One popover for every row, parented to the panel's root box.
        let context_popover = gtk::PopoverMenu::from_model(None::<&gio::MenuModel>);
        context_popover.set_parent(&side_panel);
        context_popover.set_has_arrow(true);

        Self {
            widget: side_panel,
            filter_bar,
            filter_entry,
            filter_query,
            no_matches_label,
            list_view,
            selection,
            tree,
            section_filter,
            remote_filter,
            ref_filter,
            sections,
            activate_handler: RefCell::new(None),
            ref_action_handler,
            checked_out_branch: RefCell::new(None),
            selected_ref: RefCell::new(None),
            context_ref,
            delete_action,
            context_popover,
            ref_menu,
            stash_menu,
            settings,
            batching: Cell::new(false),
        }
    }

    fn connect_signals(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.filter_entry.connect_search_changed(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.apply_filter();
            }
        });

        // Enter jumps to the first matching ref.
        let weak = Rc::downgrade(self);
        self.filter_entry.connect_activate(move |_| {
            if let Some(panel) = weak.upgrade() {
                panel.activate_first_match();
            }
        });

        // Closing the bar drops the filter so every ref is listed again.
        let weak = Rc::downgrade(self);
        self.filter_bar
            .connect_search_mode_enabled_notify(move |bar| {
                if let Some(panel) = weak.upgrade().filter(|_| !bar.is_search_mode()) {
                    panel.filter_entry.set_text("");
                    panel.apply_filter();
                }
            });

        // Rows come and go as sections expand, collapse and filter; keep the
        // selected ref's row selected whenever it is listed.
        let weak = Rc::downgrade(self);
        self.selection.connect_items_changed(move |_, _, _, _| {
            if let Some(panel) = weak.upgrade() {
                panel.sync_selection();
            }
        });

        // Moving the selection with the keyboard selects that ref without
        // switching to it; activating it (click or Enter) switches.
        let weak = Rc::downgrade(self);
        self.selection
            .connect_selection_changed(move |selection, _, _| {
                let Some(panel) = weak.upgrade().filter(|panel| !panel.batching.get()) else {
                    return;
                };
                if let Some(ref_info) = selection.selected_item().and_then(|row| ref_info(&row)) {
                    *panel.selected_ref.borrow_mut() = Some(ref_info);
                }
            });

        // Enter on a ref row, or a double click: switch to the ref. A single
        // click is handled by the row's own gesture.
        let weak = Rc::downgrade(self);
        self.list_view.connect_activate(move |_, position| {
            let Some(panel) = weak.upgrade() else { return };
            if let Some(row) = panel.tree.item(position).and_downcast::<gtk::TreeListRow>() {
                panel.activate_row(&row);
            }
        });

        // Enter or Space on a focused header expands or collapses it. The list
        // row consumes these keys even when it isn't activatable, so look at
        // them in the capture phase, before the row does.
        let weak = Rc::downgrade(self);
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(move |_, key, _, _| {
            use gtk::gdk::Key;
            let toggle_key = matches!(
                key,
                Key::Return | Key::KP_Enter | Key::ISO_Enter | Key::space | Key::KP_Space
            );
            let header_row = weak
                .upgrade()
                .filter(|_| toggle_key)
                .and_then(|panel| Some((panel.focused_header_row()?, panel)));
            match header_row {
                Some((row, panel)) => {
                    panel.toggle_header(&row);
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        self.list_view.add_controller(keys);
    }

    /// The tree row of the header that has keyboard focus, if any.
    fn focused_header_row(&self) -> Option<gtk::TreeListRow> {
        self.list_view
            .focus_child()?
            .first_child()
            .filter(|child| child.widget_name() == HEADER_ROW)?
            .first_child()
            .and_downcast::<gtk::TreeExpander>()?
            .list_row()
    }

    fn section(&self, section: Section) -> &SectionModel {
        self.sections
            .iter()
            .find(|model| model.section == section)
            .expect("every section has a model")
    }

    /// Run `f` with selection syncing paused, then sync once.
    fn batch<R>(&self, f: impl FnOnce() -> R) -> R {
        let was_batching = self.batching.replace(true);
        let result = f();
        self.batching.set(was_batching);
        if !was_batching {
            self.sync_selection();
        }
        result
    }

    /// Apply the filter entry's text to every section.
    ///
    /// While a filter is active, sections with matches are expanded (so a
    /// collapsed section can't hide them) and sections without are hidden.
    fn apply_filter(&self) {
        let query = self.filter_entry.text().trim().to_lowercase();
        let filtering = !query.is_empty();
        *self.filter_query.borrow_mut() = query;

        self.batch(|| {
            self.ref_filter.changed(gtk::FilterChange::Different);
            self.remote_filter.changed(gtk::FilterChange::Different);
            self.section_filter.changed(gtk::FilterChange::Different);

            // A section that was hidden comes back as a new, expanded row, so
            // set every section, not only those that changed.
            for i in 0..self.tree.model().n_items() {
                let Some(row) = self.tree.child_row(i) else {
                    continue;
                };
                let Some(section) = header_section(row.upcast_ref()) else {
                    continue;
                };
                row.set_expanded(filtering || self.settings.boolean(section.settings_key()));
                if filtering && section == Section::Remotes {
                    for j in 0..self.section(Section::Remotes).filtered.n_items() {
                        if let Some(remote_row) = row.child_row(j) {
                            remote_row.set_expanded(true);
                        }
                    }
                }
            }

            let any_match = self
                .sections
                .iter()
                .any(|model| model.filtered.n_items() > 0);
            self.no_matches_label.set_visible(filtering && !any_match);
        });
    }

    /// Switch to the first ref (in display order) matching the filter.
    fn activate_first_match(&self) {
        // The entry debounces `search-changed`, so apply the latest text first.
        self.apply_filter();
        if self.filter_query.borrow().is_empty() {
            return;
        }

        let first_match = Section::ALL
            .into_iter()
            .find_map(|section| self.ref_items(section, true).first().and_then(ref_info));
        if let Some(ref_info) = first_match {
            self.activate_ref(ref_info);
        }
    }

    /// The ref items of `section` in display order: all of them, or only
    /// those passing the filter.
    fn ref_items(&self, section: Section, filtered: bool) -> Vec<glib::Object> {
        let model = self.section(section);
        let top: gio::ListModel = if filtered {
            model.filtered.clone().upcast()
        } else {
            model.store.clone().upcast()
        };
        if section != Section::Remotes {
            return list_model_items(&top);
        }
        list_model_items(&top)
            .iter()
            .filter_map(header_children)
            .filter_map(|children| {
                if filtered {
                    Some(children.upcast())
                } else {
                    children.model()
                }
            })
            .flat_map(|branches| list_model_items(&branches))
            .collect()
    }

    /// Find a ref by name, preferring a branch, then a remote branch, then a
    /// tag, then a stash.
    fn find_ref(&self, name: &str) -> Option<SelectedRef> {
        [
            Section::Branches,
            Section::Remotes,
            Section::Tags,
            Section::Stashes,
        ]
        .into_iter()
        .find_map(|section| {
            self.ref_items(section, false)
                .iter()
                .find_map(|item| ref_info(item).filter(|info| info.name == name))
        })
    }

    /// Whether the panel lists `ref_info`.
    fn contains(&self, ref_info: &SelectedRef) -> bool {
        let section = match ref_info.ref_type {
            RefType::Branch => Section::Branches,
            RefType::Remote => Section::Remotes,
            RefType::Tag => Section::Tags,
            RefType::Stash => Section::Stashes,
        };
        self.ref_items(section, false)
            .iter()
            .any(|item| is_ref(item, ref_info))
    }

    fn select_ref(&self, ref_name: &str) -> bool {
        let Some(ref_info) = self.find_ref(ref_name) else {
            return false;
        };
        *self.selected_ref.borrow_mut() = Some(ref_info);
        self.sync_selection();
        true
    }

    /// Ensure a default ref is selected if nothing is currently selected.
    fn ensure_default_selection(&self) {
        let selected = self.selected_ref.borrow().clone();
        if selected.is_some_and(|ref_info| self.contains(&ref_info)) {
            return;
        }

        // Prefer main if present, otherwise the first branch, otherwise the
        // first tag.
        if self.select_ref("main") {
            return;
        }
        let first = [Section::Branches, Section::Tags]
            .into_iter()
            .find_map(|section| self.ref_items(section, false).first().and_then(ref_info));
        if let Some(ref_info) = first {
            *self.selected_ref.borrow_mut() = Some(ref_info);
            self.sync_selection();
        }
    }

    /// Select the selected ref's row if it is listed, or nothing if not.
    fn sync_selection(&self) {
        if self.batching.get() {
            return;
        }
        let target = self.selected_ref.borrow().clone();
        if let Some(target) = &target {
            if self
                .selection
                .selected_item()
                .is_some_and(|row| is_ref(&row, target))
            {
                return;
            }
        }

        let position = target.and_then(|target| {
            (0..self.tree.n_items())
                .find(|&i| self.tree.item(i).is_some_and(|row| is_ref(&row, &target)))
        });
        let position = position.unwrap_or(gtk::INVALID_LIST_POSITION);
        if self.selection.selected() != position {
            self.selection.set_selected(position);
        }
    }

    /// Switch to the ref of a ref row, or expand/collapse a header row.
    fn activate_row(&self, row: &gtk::TreeListRow) {
        match ref_info(row.upcast_ref()) {
            Some(ref_info) => self.activate_ref(ref_info),
            None => self.toggle_header(row),
        }
    }

    fn activate_ref(&self, ref_info: SelectedRef) {
        *self.selected_ref.borrow_mut() = Some(ref_info.clone());
        self.sync_selection();
        // Clone out of the cell so the handler may re-register itself.
        let handler = self.activate_handler.borrow().clone();
        if let Some(handler) = handler {
            handler(&ref_info.name, ref_info.ref_type);
        }
    }

    /// Expand or collapse a header, saving a section's state unless the
    /// filter has expanded it temporarily.
    fn toggle_header(&self, row: &gtk::TreeListRow) {
        let expanded = !row.is_expanded();
        row.set_expanded(expanded);
        if let Some(section) = header_section(row.upcast_ref()) {
            if self.filter_query.borrow().is_empty() {
                let _ = self.settings.set_boolean(section.settings_key(), expanded);
            }
        }
    }

    fn bind_list_item(self: &Rc<Self>, list_item: &gtk::ListItem) {
        let Some(row) = list_item.item().and_downcast::<gtk::TreeListRow>() else {
            return;
        };
        let Some(item) = row.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        match &*item.borrow::<PanelItem>() {
            PanelItem::Header { kind, .. } => {
                let header = self.row_widget(list_item, HEADER_ROW, build_header_row);
                update_header_row(&header, &row, kind);
                // Not activatable, so headers get no hover background; they
                // toggle by their own click gesture and the list's key handler.
                list_item.set_selectable(false);
                list_item.set_activatable(false);
            }
            PanelItem::Ref(entry) => {
                let ref_row = self.row_widget(list_item, REF_ROW, build_ref_row);
                update_ref_row(&ref_row, entry, row.depth());
                list_item.set_selectable(true);
                list_item.set_activatable(true);
            }
        }
    }

    /// The list item's row widget of the given kind, built if the recycled
    /// list item holds the other kind (or none yet).
    fn row_widget(
        self: &Rc<Self>,
        list_item: &gtk::ListItem,
        name: &str,
        build: fn(Weak<Inner>, &gtk::ListItem) -> gtk::Box,
    ) -> gtk::Box {
        if let Some(existing) = list_item
            .child()
            .filter(|child| child.widget_name() == name)
            .and_downcast::<gtk::Box>()
        {
            return existing;
        }
        let widget = build(Rc::downgrade(self), list_item);
        widget.set_widget_name(name);
        list_item.set_child(Some(&widget));
        widget
    }

    /// Show the context menu for the ref row `widget` of `list_item`.
    fn show_context_menu(
        &self,
        list_item: &gtk::ListItem,
        widget: &gtk::Widget,
        gesture: &gtk::GestureClick,
        x: f64,
        y: f64,
    ) {
        let Some(ref_info) = list_item.item().and_then(|row| ref_info(&row)) else {
            return;
        };
        let menu = match ref_info.ref_type {
            RefType::Branch | RefType::Tag => &self.ref_menu,
            RefType::Stash => &self.stash_menu,
            RefType::Remote => return,
        };
        gesture.set_state(gtk::EventSequenceState::Claimed);

        let is_checked_out = ref_info.ref_type == RefType::Branch
            && self.checked_out_branch.borrow().as_deref() == Some(ref_info.name.as_str());
        self.delete_action.set_enabled(!is_checked_out);
        *self.context_ref.borrow_mut() = Some(ref_info);

        // The popover lives on the panel root, so point at the click there.
        let Some(point) =
            widget.compute_point(&self.widget, &gtk::graphene::Point::new(x as f32, y as f32))
        else {
            return;
        };
        self.context_popover.set_menu_model(Some(menu));
        self.context_popover
            .set_pointing_to(Some(&gtk::gdk::Rectangle::new(
                point.x() as i32,
                point.y() as i32,
                1,
                1,
            )));
        self.context_popover.popup();
    }
}

// =============================================================================
// Private helper functions
// =============================================================================

/// Run `f` on the panel item behind `obj`, which is either the item or the
/// tree row holding it.
fn with_item<R>(obj: &glib::Object, f: impl FnOnce(&PanelItem) -> R) -> Option<R> {
    let item = match obj.downcast_ref::<gtk::TreeListRow>() {
        Some(row) => row.item()?,
        None => obj.clone(),
    };
    let item = item.downcast_ref::<glib::BoxedAnyObject>()?;
    let item = item.borrow::<PanelItem>();
    Some(f(&item))
}

/// The ref of a ref row or item.
fn ref_info(obj: &glib::Object) -> Option<SelectedRef> {
    with_item(obj, |item| match item {
        PanelItem::Ref(entry) => Some(entry.info.clone()),
        PanelItem::Header { .. } => None,
    })
    .flatten()
}

/// Whether `obj` is the row or item of `ref_info`.
fn is_ref(obj: &glib::Object, ref_info: &SelectedRef) -> bool {
    with_item(
        obj,
        |item| matches!(item, PanelItem::Ref(entry) if entry.info == *ref_info),
    )
    .unwrap_or(false)
}

/// The (filtered) rows under a header row or item.
fn header_children(obj: &glib::Object) -> Option<gtk::FilterListModel> {
    with_item(obj, |item| match item {
        PanelItem::Header { children, .. } => Some(children.clone()),
        PanelItem::Ref(_) => None,
    })
    .flatten()
}

/// The section a top-level header row or item stands for.
fn header_section(obj: &glib::Object) -> Option<Section> {
    with_item(obj, |item| match item {
        PanelItem::Header {
            kind: HeaderKind::Section(section),
            ..
        } => Some(*section),
        _ => None,
    })
    .flatten()
}

/// Hover text for a header: how many refs (or remotes) it holds, filter aside.
fn header_count_tooltip(obj: &glib::Object) -> Option<String> {
    with_item(obj, |item| match item {
        PanelItem::Header { kind, children } => {
            let count = children.model().map_or(0, |all| all.n_items()) as usize;
            let (singular, plural) = match kind {
                HeaderKind::Section(section) => section.count_nouns(),
                HeaderKind::Remote(_) => ("branch", "branches"),
            };
            Some(format_count_tooltip(count, singular, plural))
        }
        PanelItem::Ref(_) => None,
    })
    .flatten()
}

fn list_model_items(model: &gio::ListModel) -> Vec<glib::Object> {
    (0..model.n_items()).filter_map(|i| model.item(i)).collect()
}

/// Replace the contents of `store` with `items` in one change.
fn replace_items(store: &gio::ListStore, items: impl Iterator<Item = PanelItem>) {
    let items: Vec<glib::BoxedAnyObject> = items.map(glib::BoxedAnyObject::new).collect();
    store.splice(0, store.n_items(), &items);
}

fn branch_entry(branch: &BranchInfo, checked_out_branch: Option<&str>) -> RefEntry {
    RefEntry {
        info: SelectedRef {
            name: branch.name.clone(),
            ref_type: RefType::Branch,
        },
        label: branch.name.clone(),
        tooltip: branch.name.clone(),
        time_ago: format_time_ago(branch.latest_commit_time),
        checked_out: checked_out_branch == Some(branch.name.as_str()),
        filter_text: branch.name.clone(),
    }
}

fn remote_entry(branch: &BranchInfo) -> RefEntry {
    RefEntry {
        info: SelectedRef {
            name: branch.name.clone(),
            ref_type: RefType::Remote,
        },
        label: remote_branch_short_name(&branch.name).to_string(),
        tooltip: branch.name.clone(),
        time_ago: format_time_ago(branch.latest_commit_time),
        checked_out: false,
        filter_text: branch.name.clone(),
    }
}

fn tag_entry(tag: &TagInfo) -> RefEntry {
    RefEntry {
        info: SelectedRef {
            name: tag.name.clone(),
            ref_type: RefType::Tag,
        },
        label: tag.name.clone(),
        tooltip: tag.name.clone(),
        time_ago: format_time_ago(tag.commit_time),
        checked_out: false,
        filter_text: tag.name.clone(),
    }
}

/// A stash row shows its message; the filter matches the message too, since
/// the name is only `stash@{N}`.
fn stash_entry(stash: &StashInfo) -> RefEntry {
    RefEntry {
        info: SelectedRef {
            name: stash.name.clone(),
            ref_type: RefType::Stash,
        },
        label: stash.message.clone(),
        tooltip: format!("{}: {}", stash.name, stash.message),
        time_ago: format_time_ago(stash.time),
        checked_out: false,
        filter_text: format!("{} {}", stash.name, stash.message),
    }
}

/// Build the widget for a header row: chevron, title, and an optional "+"
/// button. Clicking anywhere but the button expands or collapses it.
fn build_header_row(panel: Weak<Inner>, list_item: &gtk::ListItem) -> gtk::Box {
    let header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["ref-panel-header"])
        .build();

    // Only draws the chevron; the header's click gesture does the toggling,
    // so a click on the chevron must reach it. The list row's own 2px padding
    // makes up the rest of the 10px inset.
    let expander = gtk::TreeExpander::builder()
        .indent_for_depth(false)
        .can_target(false)
        .margin_start(8)
        .build();
    header.append(&expander);

    let label_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["ref-panel-expander-label"])
        .build();
    let title = gtk::Label::builder().halign(gtk::Align::Start).build();
    title.add_css_class("heading");
    label_box.append(&title);
    append_header_button(&label_box);
    header.append(&label_box);

    // The count is read on hover, so it is current after every refresh.
    let list_item_for_tooltip = list_item.downgrade();
    label_box.set_has_tooltip(true);
    label_box.connect_query_tooltip(move |_, _, _, _, tooltip| {
        let text = list_item_for_tooltip
            .upgrade()
            .and_then(|list_item| list_item.item())
            .and_then(|row| header_count_tooltip(&row));
        tooltip.set_text(text.as_deref());
        text.is_some()
    });

    // Every click toggles, so a double click opens and closes again.
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_PRIMARY);
    let list_item = list_item.downgrade();
    click.connect_released(move |_, _, _, _| {
        let row = list_item
            .upgrade()
            .and_then(|list_item| list_item.item())
            .and_downcast::<gtk::TreeListRow>();
        if let (Some(panel), Some(row)) = (panel.upgrade(), row) {
            panel.toggle_header(&row);
        }
    });
    header.add_controller(click);
    header
}

fn update_header_row(header: &gtk::Box, row: &gtk::TreeListRow, kind: &HeaderKind) {
    let Some(expander) = header.first_child().and_downcast::<gtk::TreeExpander>() else {
        return;
    };
    expander.set_list_row(Some(row));
    let Some(label_box) = expander.next_sibling() else {
        return;
    };
    let Some(title) = label_box.first_child().and_downcast::<gtk::Label>() else {
        return;
    };
    let Some(button) = title.next_sibling().and_downcast::<gtk::Button>() else {
        return;
    };

    let (text, create_button, indent) = match kind {
        HeaderKind::Section(section) => (section.title(), section.create_button(), 0),
        HeaderKind::Remote(remote) => (remote.as_str(), None, REMOTE_INDENT),
    };
    title.set_text(text);
    header.set_margin_start(indent);
    button.set_visible(create_button.is_some());
    button.set_action_name(create_button.map(|(_, action)| action));
    button.set_tooltip_text(create_button.map(|(tooltip, _)| tooltip));
}

/// Build the widget for a ref row: checkmark, name, and age. A click switches
/// to the ref; a right click opens its context menu.
fn build_ref_row(panel: Weak<Inner>, list_item: &gtk::ListItem) -> gtk::Box {
    let row_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_start(12)
        .margin_end(12)
        .margin_top(6)
        .margin_bottom(6)
        .spacing(8)
        .hexpand(true)
        .build();

    // Checkmark on the checked-out branch; a blank placeholder on other rows
    // keeps the names aligned.
    let check_icon = gtk::Image::from_icon_name("object-select-symbolic");
    check_icon.set_pixel_size(16);
    row_box.append(&check_icon);

    let name_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    row_box.append(&name_label);

    let time_label = gtk::Label::builder()
        .halign(gtk::Align::End)
        .hexpand(true)
        .build();
    time_label.add_css_class("dim-label");
    row_box.append(&time_label);

    // A single click switches to the ref; selection itself is left to the
    // list view. Enter and double clicks arrive as list activation.
    let click = gtk::GestureClick::new();
    click.set_button(gtk::gdk::BUTTON_PRIMARY);
    let (panel_for_click, list_item_for_click) = (panel.clone(), list_item.downgrade());
    click.connect_released(move |_, n_press, _, _| {
        if n_press != 1 {
            return;
        }
        let row = list_item_for_click
            .upgrade()
            .and_then(|list_item| list_item.item())
            .and_downcast::<gtk::TreeListRow>();
        if let (Some(panel), Some(row)) = (panel_for_click.upgrade(), row) {
            panel.activate_row(&row);
        }
    });
    row_box.add_controller(click);

    let right_click = gtk::GestureClick::new();
    right_click.set_button(gtk::gdk::BUTTON_SECONDARY);
    let list_item = list_item.downgrade();
    right_click.connect_pressed(move |gesture, _, x, y| {
        let (Some(panel), Some(list_item), Some(widget)) =
            (panel.upgrade(), list_item.upgrade(), gesture.widget())
        else {
            return;
        };
        panel.show_context_menu(&list_item, &widget, gesture, x, y);
    });
    row_box.add_controller(right_click);
    row_box
}

fn update_ref_row(row_box: &gtk::Box, entry: &RefEntry, depth: u32) {
    // Remote branches sit under their remote's header (depth 2).
    let indent = if depth >= 2 { REMOTE_INDENT } else { 0 };
    row_box.set_margin_start(12 + indent);

    let Some(check_icon) = row_box.first_child().and_downcast::<gtk::Image>() else {
        return;
    };
    let Some(name_label) = check_icon.next_sibling().and_downcast::<gtk::Label>() else {
        return;
    };
    let Some(time_label) = name_label.next_sibling().and_downcast::<gtk::Label>() else {
        return;
    };

    check_icon.set_opacity(if entry.checked_out { 1.0 } else { 0.0 });
    check_icon.set_tooltip_text(entry.checked_out.then_some("Checked out branch"));
    name_label.set_text(&entry.label);
    name_label.set_tooltip_text(Some(&entry.tooltip));
    time_label.set_markup(&format!(
        "<span size='small'>{}</span>",
        glib::markup_escape_text(&entry.time_ago)
    ));
}

/// Group remote-tracking branches by remote name (the part before the first `/`).
fn group_remote_branches(remote_branches: &[BranchInfo]) -> Vec<(String, Vec<BranchInfo>)> {
    let mut by_remote: BTreeMap<String, Vec<BranchInfo>> = BTreeMap::new();

    for branch in remote_branches {
        let Some((remote, _)) = branch.name.split_once('/') else {
            continue;
        };
        by_remote
            .entry(remote.to_string())
            .or_default()
            .push(branch.clone());
    }

    let mut groups: Vec<(String, Vec<BranchInfo>)> = by_remote.into_iter().collect();
    for (_, branches) in &mut groups {
        branches.sort_by(|a, b| b.latest_commit_time.cmp(&a.latest_commit_time));
    }
    groups
}

fn remote_branch_short_name(full_name: &str) -> &str {
    full_name
        .split_once('/')
        .map(|(_, b)| b)
        .unwrap_or(full_name)
}

/// Case-insensitive substring match. `query` must already be lowercased.
fn filter_matches(text: &str, query: &str) -> bool {
    text.to_lowercase().contains(query)
}

/// Format a hover tooltip describing how many refs are in a section.
fn format_count_tooltip(count: usize, singular: &str, plural: &str) -> String {
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {plural}")
    }
}

/// Add a small "+" button to the end of a section header. Its action and
/// tooltip are set per section when the row is bound.
fn append_header_button(label_box: &gtk::Box) {
    label_box.set_hexpand(true);
    let button = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .halign(gtk::Align::End)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .css_classes(["flat", "ref-panel-header-button"])
        .build();
    label_box.append(&button);
}

/// Install the `refs.*` actions on the panel. `refs.delete` acts on
/// `context_ref`, the row that was last right-clicked. Returns the delete
/// action so the context menu can disable it for the checked-out branch.
fn install_ref_actions(
    panel: &gtk::Box,
    handler: &Rc<RefCell<Option<Rc<dyn Fn(RefAction)>>>>,
    context_ref: &Rc<RefCell<Option<SelectedRef>>>,
) -> gio::SimpleAction {
    let emit = {
        let handler = handler.clone();
        move |action: RefAction| {
            // Clone out of the cell so the handler may re-register itself.
            let handler = handler.borrow().clone();
            if let Some(handler) = handler {
                handler(action);
            }
        }
    };

    let action_group = gio::SimpleActionGroup::new();

    let new_branch = gio::SimpleAction::new("new-branch", None);
    let emit_new_branch = emit.clone();
    new_branch.connect_activate(move |_, _| emit_new_branch(RefAction::NewBranch));
    action_group.add_action(&new_branch);

    let new_tag = gio::SimpleAction::new("new-tag", None);
    let emit_new_tag = emit.clone();
    new_tag.connect_activate(move |_, _| emit_new_tag(RefAction::NewTag));
    action_group.add_action(&new_tag);

    let delete = gio::SimpleAction::new("delete", None);
    let context_ref = context_ref.clone();
    delete.connect_activate(move |_, _| {
        let target = context_ref.borrow_mut().take();
        if let Some(SelectedRef { name, ref_type }) = target {
            emit(RefAction::Delete { name, ref_type });
        }
    });
    action_group.add_action(&delete);

    panel.insert_action_group("refs", Some(&action_group));
    delete
}

/// Sort branches with "main" or "master" first, then by latest commit time.
fn sort_branches(branches: &[BranchInfo]) -> Vec<BranchInfo> {
    let mut sorted = branches.to_vec();
    sorted.sort_by(|a, b| {
        branch_priority(&a.name)
            .cmp(&branch_priority(&b.name))
            .then_with(|| {
                b.latest_commit_time
                    .cmp(&a.latest_commit_time)
                    .then_with(|| natural_compare(&a.name, &b.name))
            })
    });
    sorted
}

fn branch_priority(name: &str) -> u8 {
    match name {
        "main" => 0,
        "master" => 1,
        _ => 2,
    }
}

/// Sort tags alphabetically using natural sort (reverse order so newest versions appear first).
pub(crate) fn sort_tags(tags: &[TagInfo]) -> Vec<TagInfo> {
    let mut sorted = tags.to_vec();
    sorted.sort_by(|a, b| natural_compare(&b.name, &a.name));
    sorted
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
    format_duration_ago(Utc::now().signed_duration_since(dt))
}

/// Format an elapsed duration using the largest unit that is at least 1.
fn format_duration_ago(duration: chrono::Duration) -> String {
    let total_seconds = duration.num_seconds();
    let total_minutes = duration.num_minutes();
    let total_hours = duration.num_hours();
    let total_days = duration.num_days();

    if total_seconds < 60 {
        "just now".to_string()
    } else if total_minutes < 60 {
        format!("{}m", total_minutes)
    } else if total_hours < 24 {
        format!("{}h", total_hours)
    } else if total_days < 7 {
        format!("{}d", total_days)
    } else if total_days < 30 {
        format!("{}w", total_days / 7)
    } else if total_days < 365 {
        // Twelve 30-day months is only 360 days, so days 360-364 are capped
        // at 11 months instead of rolling over to "12mo" or "0y".
        format!("{}mo", (total_days / 30).min(11))
    } else {
        format!("{}y", total_days / 365)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn branch(name: &str, timestamp: i64) -> BranchInfo {
        BranchInfo {
            name: name.to_string(),
            latest_commit_time: Utc.timestamp_opt(timestamp, 0).unwrap(),
        }
    }

    #[test]
    fn sort_branches_orders_newest_first() {
        let sorted = sort_branches(&[
            branch("oldest", 100),
            branch("newest", 300),
            branch("middle", 200),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["newest", "middle", "oldest"]);
    }

    #[test]
    fn sort_branches_pins_main_above_newer_branches() {
        let sorted = sort_branches(&[
            branch("main", 100),
            branch("feature", 300),
            branch("bugfix", 200),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["main", "feature", "bugfix"]);
    }

    #[test]
    fn sort_branches_pins_master_above_newer_branches() {
        let sorted = sort_branches(&[
            branch("feature", 300),
            branch("master", 100),
            branch("bugfix", 200),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["master", "feature", "bugfix"]);
    }

    #[test]
    fn sort_branches_orders_main_before_master() {
        let sorted = sort_branches(&[
            branch("master", 300),
            branch("main", 100),
            branch("feature", 200),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["main", "master", "feature"]);
    }

    #[test]
    fn sort_branches_uses_natural_name_order_for_ties() {
        let sorted = sort_branches(&[
            branch("branch-10", 100),
            branch("branch-2", 100),
            branch("branch-1", 100),
        ]);

        let names: Vec<_> = sorted.iter().map(|branch| branch.name.as_str()).collect();
        assert_eq!(names, ["branch-1", "branch-2", "branch-10"]);
    }

    #[test]
    fn filter_matches_ignores_case_of_the_ref_name() {
        assert!(filter_matches("Feature/Login-Page", "login"));
        assert!(filter_matches("v2026.6", "v2026"));
        assert!(filter_matches(
            "stash@{0} WIP on main: fix crash",
            "on main"
        ));
    }

    #[test]
    fn filter_matches_rejects_refs_without_the_query() {
        assert!(!filter_matches("feature/login", "logout"));
        assert!(!filter_matches("main", "maintenance"));
    }

    #[test]
    fn branch_entry_marks_only_the_checked_out_branch() {
        let main = branch("main", 100);
        assert!(branch_entry(&main, Some("main")).checked_out);
        assert!(!branch_entry(&main, Some("feature")).checked_out);
        assert!(!branch_entry(&main, None).checked_out);
    }

    #[test]
    fn remote_entry_shows_branch_without_remote_but_keeps_full_name() {
        let entry = remote_entry(&branch("origin/feature/login", 100));
        assert_eq!(entry.label, "feature/login");
        assert_eq!(entry.tooltip, "origin/feature/login");
        assert_eq!(
            entry.info,
            SelectedRef {
                name: "origin/feature/login".to_string(),
                ref_type: RefType::Remote,
            }
        );
        assert!(filter_matches(&entry.filter_text, "origin/"));
    }

    #[test]
    fn stash_entry_shows_message_and_filters_on_name_and_message() {
        let entry = stash_entry(&StashInfo {
            name: "stash@{0}".to_string(),
            message: "WIP on main: fix crash".to_string(),
            sha: "abc".to_string(),
            time: Utc.timestamp_opt(100, 0).unwrap(),
        });
        assert_eq!(entry.label, "WIP on main: fix crash");
        assert_eq!(entry.tooltip, "stash@{0}: WIP on main: fix crash");
        assert_eq!(entry.info.name, "stash@{0}");
        assert!(filter_matches(&entry.filter_text, "stash@{0}"));
        assert!(filter_matches(&entry.filter_text, "fix crash"));
    }

    #[test]
    fn group_remote_branches_groups_by_remote_newest_first() {
        let groups = group_remote_branches(&[
            branch("upstream/main", 100),
            branch("origin/old", 100),
            branch("origin/new", 300),
            branch("no-remote", 200),
        ]);
        let names: Vec<(&str, Vec<&str>)> = groups
            .iter()
            .map(|(remote, branches)| {
                (
                    remote.as_str(),
                    branches.iter().map(|b| b.name.as_str()).collect(),
                )
            })
            .collect();
        assert_eq!(
            names,
            [
                ("origin", vec!["origin/new", "origin/old"]),
                ("upstream", vec!["upstream/main"]),
            ]
        );
    }

    fn ago_days(days: i64) -> String {
        format_duration_ago(chrono::Duration::days(days))
    }

    #[test]
    fn format_duration_ago_uses_small_units_under_a_week() {
        assert_eq!(
            format_duration_ago(chrono::Duration::seconds(-5)),
            "just now"
        );
        assert_eq!(
            format_duration_ago(chrono::Duration::seconds(59)),
            "just now"
        );
        assert_eq!(format_duration_ago(chrono::Duration::seconds(60)), "1m");
        assert_eq!(format_duration_ago(chrono::Duration::minutes(59)), "59m");
        assert_eq!(format_duration_ago(chrono::Duration::hours(1)), "1h");
        assert_eq!(format_duration_ago(chrono::Duration::hours(23)), "23h");
        assert_eq!(ago_days(1), "1d");
        assert_eq!(ago_days(6), "6d");
    }

    #[test]
    fn format_duration_ago_switches_units_at_each_boundary() {
        assert_eq!(ago_days(7), "1w");
        assert_eq!(ago_days(29), "4w");
        assert_eq!(ago_days(30), "1mo");
        assert_eq!(ago_days(359), "11mo");
        assert_eq!(ago_days(365), "1y");
        assert_eq!(ago_days(729), "1y");
        assert_eq!(ago_days(730), "2y");
    }

    #[test]
    fn format_duration_ago_shows_months_for_the_last_days_before_a_year() {
        for days in 360..365 {
            assert_eq!(ago_days(days), "11mo", "{days} days");
        }
    }

    #[test]
    fn format_duration_ago_never_shows_a_zero_count() {
        for days in 0..=800 {
            let formatted = ago_days(days);
            assert!(
                formatted == "just now" || !formatted.starts_with('0'),
                "{days} days formatted as {formatted}"
            );
        }
    }

    #[test]
    fn format_count_tooltip_uses_singular_for_one() {
        assert_eq!(format_count_tooltip(1, "branch", "branches"), "1 branch");
        assert_eq!(format_count_tooltip(1, "tag", "tags"), "1 tag");
        assert_eq!(format_count_tooltip(1, "remote", "remotes"), "1 remote");
    }

    #[test]
    fn format_count_tooltip_uses_plural_for_zero_and_many() {
        assert_eq!(format_count_tooltip(0, "branch", "branches"), "0 branches");
        assert_eq!(format_count_tooltip(12, "tag", "tags"), "12 tags");
        assert_eq!(format_count_tooltip(2, "remote", "remotes"), "2 remotes");
    }
}
