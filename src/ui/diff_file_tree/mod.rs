use gtk::{gio, glib, prelude::*};

use std::cell::RefCell;
use std::rc::Rc;

/// One changed file as listed in the file tree.
#[derive(Debug, Clone, PartialEq)]
pub struct FileSummary {
    /// Position of the file in the commit diff (matches the diff's expander order).
    pub index: usize,
    /// Path used to place the file in the tree (the new path for renames).
    pub path: String,
    /// Full label shown as a tooltip (e.g. "old → new" for renames).
    pub label: String,
    pub additions: usize,
    pub deletions: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TreeNode {
    Dir {
        name: String,
        children: Vec<TreeNode>,
    },
    File(FileSummary),
}

/// Builds a directory tree from the changed files, preserving their order.
///
/// Chains of directories that only contain a single sub-directory are
/// collapsed into one node (e.g. `app/routes`) to keep the tree compact.
pub fn build_tree(files: &[FileSummary]) -> Vec<TreeNode> {
    let mut root: Vec<TreeNode> = Vec::new();
    for file in files {
        let mut parts: Vec<&str> = file.path.split('/').filter(|p| !p.is_empty()).collect();
        parts.pop(); // The file name itself.
        insert_file(&mut root, &parts, file.clone());
    }
    root.into_iter().map(compress_dirs).collect()
}

fn insert_file(nodes: &mut Vec<TreeNode>, dirs: &[&str], file: FileSummary) {
    let Some((first, rest)) = dirs.split_first() else {
        nodes.push(TreeNode::File(file));
        return;
    };
    let is_dir = |node: &TreeNode| matches!(node, TreeNode::Dir { name, .. } if name == first);
    // Diffs are path-sorted, so the matching directory is almost always the
    // last node; fall back to a full search otherwise.
    let idx = if nodes.last().is_some_and(is_dir) {
        nodes.len() - 1
    } else if let Some(idx) = nodes.iter().position(is_dir) {
        idx
    } else {
        nodes.push(TreeNode::Dir {
            name: first.to_string(),
            children: Vec::new(),
        });
        nodes.len() - 1
    };
    if let TreeNode::Dir { children, .. } = &mut nodes[idx] {
        insert_file(children, rest, file);
    }
}

fn compress_dirs(node: TreeNode) -> TreeNode {
    let TreeNode::Dir {
        mut name,
        mut children,
    } = node
    else {
        return node;
    };
    while children.len() == 1 && matches!(children[0], TreeNode::Dir { .. }) {
        let Some(TreeNode::Dir {
            name: child_name,
            children: grandchildren,
        }) = children.pop()
        else {
            unreachable!();
        };
        name = format!("{name}/{child_name}");
        children = grandchildren;
    }
    TreeNode::Dir {
        name,
        children: children.into_iter().map(compress_dirs).collect(),
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

fn store_for_nodes(nodes: &[TreeNode]) -> gio::ListStore {
    let store = gio::ListStore::new::<glib::BoxedAnyObject>();
    for node in nodes {
        store.append(&glib::BoxedAnyObject::new(node.clone()));
    }
    store
}

fn node_for_row(row: &gtk::TreeListRow) -> Option<TreeNode> {
    let boxed = row.item().and_downcast::<glib::BoxedAnyObject>()?;
    let node = boxed.borrow::<TreeNode>();
    Some(node.clone())
}

type FileActivatedCallback = Rc<RefCell<Option<Rc<dyn Fn(usize)>>>>;

/// Handles a click/activation on a tree row: directories toggle open/closed,
/// files notify the registered callback with their diff index.
fn activate_row(row: &gtk::TreeListRow, on_file_activated: &FileActivatedCallback) {
    match node_for_row(row) {
        Some(TreeNode::Dir { .. }) => row.set_expanded(!row.is_expanded()),
        Some(TreeNode::File(file)) => {
            let callback = on_file_activated.borrow().clone();
            if let Some(callback) = callback {
                callback(file.index);
            }
        }
        None => {}
    }
}

/// Small colored "+N" / "−N" chip.
fn count_label(css_class: &str) -> gtk::Label {
    let label = gtk::Label::builder().valign(gtk::Align::Center).build();
    label.add_css_class("diff-stat");
    label.add_css_class(css_class);
    label
}

fn set_count_label(label: &gtk::Label, prefix: &str, count: usize) {
    label.set_text(&format!("{prefix}{count}"));
    label.set_visible(count > 0);
}

/// Sidebar listing the files changed in a diff as a directory tree, with the
/// number of added/removed lines per file.
#[derive(Clone)]
pub struct DiffFileTree {
    pub widget: gtk::Box,
    added_label: gtk::Label,
    removed_label: gtk::Label,
    root_store: gio::ListStore,
    on_file_activated: FileActivatedCallback,
}

impl DiffFileTree {
    pub fn new() -> Self {
        let on_file_activated: FileActivatedCallback = Rc::new(RefCell::new(None));

        let title_label = gtk::Label::builder()
            .label("Lines updated")
            .halign(gtk::Align::Start)
            .hexpand(true)
            .build();
        title_label.add_css_class("heading");

        let added_label = count_label("diff-stat-add");
        let removed_label = count_label("diff-stat-remove");

        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(4)
            .margin_start(12)
            .margin_end(12)
            .margin_top(8)
            .margin_bottom(6)
            .build();
        header.append(&title_label);
        header.append(&added_label);
        header.append(&removed_label);

        let root_store = gio::ListStore::new::<glib::BoxedAnyObject>();
        let tree_model = gtk::TreeListModel::new(root_store.clone(), false, true, |item| {
            let boxed = item.downcast_ref::<glib::BoxedAnyObject>()?;
            let node = boxed.borrow::<TreeNode>();
            match &*node {
                TreeNode::Dir { children, .. } => Some(store_for_nodes(children).upcast()),
                TreeNode::File(_) => None,
            }
        });
        let selection = gtk::SingleSelection::builder()
            .model(&tree_model)
            .autoselect(false)
            .can_unselect(true)
            .build();

        let factory = gtk::SignalListItemFactory::new();
        let on_file_activated_for_setup = on_file_activated.clone();
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();

            let name_label = gtk::Label::builder()
                .halign(gtk::Align::Start)
                .hexpand(true)
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .build();
            let add_label = count_label("diff-stat-add");
            let remove_label = count_label("diff-stat-remove");

            let content = gtk::Box::builder()
                .orientation(gtk::Orientation::Horizontal)
                .spacing(6)
                .build();
            content.append(&name_label);
            content.append(&add_label);
            content.append(&remove_label);

            // Single click on the row content: toggle directories, jump to files.
            let gesture = gtk::GestureClick::new();
            gesture.set_button(1);
            let item_weak = item.downgrade();
            let on_file_activated = on_file_activated_for_setup.clone();
            gesture.connect_released(move |_, _, _, _| {
                let Some(item) = item_weak.upgrade() else {
                    return;
                };
                if let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() {
                    activate_row(&row, &on_file_activated);
                }
            });
            content.add_controller(gesture);

            let expander = gtk::TreeExpander::new();
            expander.set_child(Some(&content));
            item.set_child(Some(&expander));
        });

        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(row) = item.item().and_downcast::<gtk::TreeListRow>() else {
                return;
            };
            let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() else {
                return;
            };
            expander.set_list_row(Some(&row));
            let Some(content) = expander.child().and_downcast::<gtk::Box>() else {
                return;
            };
            let name_label = content.first_child().and_downcast::<gtk::Label>().unwrap();
            let add_label = name_label
                .next_sibling()
                .and_downcast::<gtk::Label>()
                .unwrap();
            let remove_label = add_label
                .next_sibling()
                .and_downcast::<gtk::Label>()
                .unwrap();

            match node_for_row(&row) {
                Some(TreeNode::Dir { name, .. }) => {
                    // Directories are toggled by click, never selected.
                    item.set_selectable(false);
                    name_label.set_text(&name);
                    name_label.set_tooltip_text(Some(&name));
                    name_label.remove_css_class("diff-file-tree-file");
                    add_label.set_visible(false);
                    remove_label.set_visible(false);
                }
                Some(TreeNode::File(file)) => {
                    item.set_selectable(true);
                    name_label.set_text(file_name(&file.path));
                    name_label.set_tooltip_text(Some(&file.label));
                    name_label.add_css_class("diff-file-tree-file");
                    set_count_label(&add_label, "+", file.additions);
                    set_count_label(&remove_label, "\u{2212}", file.deletions);
                }
                None => {}
            }
        });

        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(expander) = item.child().and_downcast::<gtk::TreeExpander>() {
                expander.set_list_row(None);
            }
        });

        let list_view = gtk::ListView::new(Some(selection), Some(factory));
        list_view.add_css_class("navigation-sidebar");
        list_view.add_css_class("diff-file-tree-list");

        // Keyboard activation (Enter) behaves like a click.
        let on_file_activated_for_activate = on_file_activated.clone();
        let tree_model_for_activate = tree_model.clone();
        list_view.connect_activate(move |_, position| {
            if let Some(row) = tree_model_for_activate.row(position) {
                activate_row(&row, &on_file_activated_for_activate);
            }
        });

        let scrolled = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .child(&list_view)
            .build();

        let widget = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .vexpand(true)
            .build();
        widget.add_css_class("diff-file-tree");
        widget.append(&header);
        widget.append(&scrolled);

        let tree = Self {
            widget,
            added_label,
            removed_label,
            root_store,
            on_file_activated,
        };
        tree.clear();
        tree
    }

    /// Replace the listed files.
    pub fn set_files(&self, files: &[FileSummary]) {
        let additions: usize = files.iter().map(|f| f.additions).sum();
        let deletions: usize = files.iter().map(|f| f.deletions).sum();
        set_count_label(&self.added_label, "+", additions);
        set_count_label(&self.removed_label, "\u{2212}", deletions);

        let nodes: Vec<glib::BoxedAnyObject> = build_tree(files)
            .into_iter()
            .map(glib::BoxedAnyObject::new)
            .collect();
        self.root_store.splice(0, self.root_store.n_items(), &nodes);
    }

    /// Remove all files (e.g. while a new diff is loading).
    pub fn clear(&self) {
        self.root_store.remove_all();
        self.added_label.set_visible(false);
        self.removed_label.set_visible(false);
    }

    /// Register the callback invoked with a file's diff index when it is clicked.
    pub fn on_file_activated<F: Fn(usize) + 'static>(&self, callback: F) {
        *self.on_file_activated.borrow_mut() = Some(Rc::new(callback));
    }
}

impl Default for DiffFileTree {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(index: usize, path: &str) -> FileSummary {
        FileSummary {
            index,
            path: path.to_string(),
            label: path.to_string(),
            additions: 1,
            deletions: 0,
        }
    }

    fn dir(name: &str, children: Vec<TreeNode>) -> TreeNode {
        TreeNode::Dir {
            name: name.to_string(),
            children,
        }
    }

    #[test]
    fn build_tree_nests_files_under_directories_in_order() {
        let files = vec![
            file(0, "README.md"),
            file(1, "src/main.rs"),
            file(2, "src/ui/mod.rs"),
            file(3, "src/util.rs"),
        ];
        assert_eq!(
            build_tree(&files),
            vec![
                TreeNode::File(files[0].clone()),
                dir(
                    "src",
                    vec![
                        TreeNode::File(files[1].clone()),
                        dir("ui", vec![TreeNode::File(files[2].clone())]),
                        TreeNode::File(files[3].clone()),
                    ]
                ),
            ]
        );
    }

    #[test]
    fn build_tree_compresses_single_child_directory_chains() {
        let files = vec![
            file(0, "app/routes/index.tsx"),
            file(1, "app/routes/new.tsx"),
            file(2, "tests/e2e/a11y.spec.ts"),
        ];
        assert_eq!(
            build_tree(&files),
            vec![
                dir(
                    "app/routes",
                    vec![
                        TreeNode::File(files[0].clone()),
                        TreeNode::File(files[1].clone()),
                    ]
                ),
                dir("tests/e2e", vec![TreeNode::File(files[2].clone())]),
            ]
        );
    }

    #[test]
    fn build_tree_does_not_compress_directory_with_files_and_subdirs() {
        let files = vec![file(0, "server/init/lti.ts"), file(1, "server/root.ts")];
        assert_eq!(
            build_tree(&files),
            vec![dir(
                "server",
                vec![
                    dir("init", vec![TreeNode::File(files[0].clone())]),
                    TreeNode::File(files[1].clone()),
                ]
            )]
        );
    }

    #[test]
    fn build_tree_merges_non_contiguous_directory_entries() {
        let files = vec![file(0, "a/x.rs"), file(1, "b.rs"), file(2, "a/y.rs")];
        assert_eq!(
            build_tree(&files),
            vec![
                dir(
                    "a",
                    vec![
                        TreeNode::File(files[0].clone()),
                        TreeNode::File(files[2].clone()),
                    ]
                ),
                TreeNode::File(files[1].clone()),
            ]
        );
    }

    #[test]
    fn build_tree_empty_input_is_empty() {
        assert!(build_tree(&[]).is_empty());
    }

    #[test]
    fn file_name_returns_last_component() {
        assert_eq!(file_name("src/ui/mod.rs"), "mod.rs");
        assert_eq!(file_name("README.md"), "README.md");
    }
}
