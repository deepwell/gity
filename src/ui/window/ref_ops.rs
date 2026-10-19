//! Dialogs for creating branches and tags and for deleting branches, tags,
//! and stashes. Requests come from the branch panel (see `RefAction`).

use adw::prelude::*;

use crate::git::{self, NewRefKind};
use crate::ui::RefType;

use super::APP_NAME;
use super::repo;
use super::state::AppState;
use super::ui::WindowUi;

/// Ask for the name (and, for tags, an optional message) of a new branch or
/// tag, then create it at the selected commit or, failing that, at the tip of
/// the viewed ref.
pub fn show_create_dialog(
    window: &gtk::ApplicationWindow,
    ui: &WindowUi,
    state: &AppState,
    kind: NewRefKind,
) {
    let Some(path) = state.current_path.borrow().clone() else {
        return;
    };
    let target_sha = ui.repo_view.commit_list.selected_commit_sha().or_else(|| {
        let current_ref = state.current_ref.borrow().clone()?;
        git::resolve_ref_sha(&path, &current_ref)
    });
    let Some(target_sha) = target_sha else {
        ui.show_toast(&format!(
            "There is no commit to create the {} at",
            kind.label()
        ));
        return;
    };

    let heading = match kind {
        NewRefKind::Branch => "New Branch",
        NewRefKind::Tag => "New Tag",
    };
    let dialog = adw::AlertDialog::new(Some(heading), None);

    let fields = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let name_row = adw::EntryRow::builder()
        .title("Name")
        .activates_default(true)
        .build();
    fields.append(&name_row);
    let message_row = (kind == NewRefKind::Tag).then(|| {
        let row = adw::EntryRow::builder()
            .title("Message (optional)")
            .activates_default(true)
            .build();
        fields.append(&row);
        row
    });

    let summary = git::get_commit_metadata(&path.to_string_lossy(), &target_sha)
        .map(|m| m.commit_message.lines().next().unwrap_or("").to_string())
        .unwrap_or_default();
    let target_label = gtk::Label::builder()
        .label(format!("At {} {summary}", short_sha(&target_sha)))
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label", "caption"])
        .build();
    let error_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .wrap(true)
        .visible(false)
        .css_classes(["error", "caption"])
        .build();

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .build();
    content.append(&fields);
    content.append(&target_label);
    content.append(&error_label);
    dialog.set_extra_child(Some(&content));

    dialog.add_responses(&[("cancel", "Cancel"), ("create", "Create")]);
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("create"));
    dialog.set_close_response("cancel");
    dialog.set_response_enabled("create", false);

    // Validate as the user types. An empty name only disables Create; the
    // "Name is required" message would be noise before anything is typed.
    let dialog_for_validate = dialog.clone();
    let path_for_validate = path.clone();
    name_row.connect_changed(move |row| {
        let name = row.text();
        let result = git::validate_new_ref_name(&path_for_validate, kind, &name);
        dialog_for_validate.set_response_enabled("create", result.is_ok());
        match result {
            Err(message) if !name.is_empty() => {
                error_label.set_label(&message);
                error_label.set_visible(true);
            }
            _ => error_label.set_visible(false),
        }
    });

    let ui = ui.clone();
    let state = state.clone();
    let name_row_for_response = name_row.clone();
    dialog.connect_response(Some("create"), move |_, _| {
        let name = name_row_for_response.text().to_string();
        let message = message_row
            .as_ref()
            .map(|row| row.text().trim().to_string())
            .filter(|message| !message.is_empty());
        let result = match kind {
            NewRefKind::Branch => git::create_branch(&path, &name, &target_sha),
            NewRefKind::Tag => {
                // Normalize like `git tag -m` (trailing newline, no stray whitespace).
                let message = message.map(|m| git2::message_prettify(m, None)).transpose();
                message.and_then(|message| {
                    git::create_tag(&path, &name, &target_sha, message.as_deref())
                })
            }
        };
        match result {
            Ok(()) => {
                repo::refresh_repo(&ui, &state, APP_NAME);
                ui.show_toast(&format!("Created {} “{name}”", kind.label()));
            }
            Err(e) => ui.show_toast(&format!(
                "Couldn't create {}: {}",
                kind.label(),
                e.message()
            )),
        }
    });

    dialog.set_focus(Some(&name_row));
    dialog.present(Some(window));
}

/// Ask for confirmation, then delete a local branch, tag, or stash.
pub fn confirm_delete(
    window: &gtk::ApplicationWindow,
    ui: &WindowUi,
    state: &AppState,
    name: &str,
    ref_type: RefType,
) {
    let Some(path) = state.current_path.borrow().clone() else {
        return;
    };

    let (heading, body, stash_sha) = match ref_type {
        RefType::Branch => {
            let mut body = format!("“{name}” will be permanently deleted.");
            if !git::is_branch_merged_into_head(&path, name) {
                let into = git::checked_out_branch_name(&path)
                    .map(|branch| format!("“{branch}”"))
                    .unwrap_or_else(|| "HEAD".to_string());
                body.push_str(&format!(
                    " It has commits that are not merged into {into} and may be lost."
                ));
            }
            ("Delete Branch?", body, None)
        }
        RefType::Tag => (
            "Delete Tag?",
            format!("“{name}” will be permanently deleted."),
            None,
        ),
        RefType::Stash => {
            // Resolve the stash now and delete it by SHA, so the right entry
            // is dropped even if the stash indices change meanwhile.
            let stash = git::get_stash_list(&path)
                .ok()
                .and_then(|stashes| stashes.into_iter().find(|s| s.name == name));
            let Some(stash) = stash else {
                ui.show_toast("The stash no longer exists");
                repo::refresh_repo(ui, state, APP_NAME);
                return;
            };
            (
                "Delete Stash?",
                format!(
                    "“{}” ({name}) will be permanently deleted. Its changes cannot be recovered.",
                    stash.message
                ),
                Some(stash.sha),
            )
        }
        RefType::Remote => return,
    };

    let dialog = adw::AlertDialog::new(Some(heading), Some(&body));
    dialog.add_responses(&[("cancel", "Cancel"), ("delete", "Delete")]);
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");

    let ui = ui.clone();
    let state = state.clone();
    let name = name.to_string();
    dialog.connect_response(Some("delete"), move |_, _| {
        let is_viewed = match &stash_sha {
            Some(sha) => ui
                .repo_view
                .commit_list
                .stash_head()
                .is_some_and(|(_, viewed_sha)| &viewed_sha == sha),
            None => {
                state.current_ref.borrow().as_deref() == Some(name.as_str())
                    && *state.current_ref_type.borrow() == Some(ref_type)
            }
        };

        let result = match (ref_type, &stash_sha) {
            (RefType::Branch, _) => git::delete_branch(&path, &name),
            (RefType::Tag, _) => git::delete_tag(&path, &name),
            (RefType::Stash, Some(sha)) => git::delete_stash(&path, sha),
            _ => return,
        };
        let what = ref_type_label(ref_type);
        match result {
            Ok(()) => {
                if is_viewed {
                    // The viewed ref is gone; show the default branch rather
                    // than letting a refresh report it (and the selection) missing.
                    repo::load_repo(&ui, &state, APP_NAME, path.clone(), None);
                } else {
                    repo::refresh_repo(&ui, &state, APP_NAME);
                }
                if ref_type == RefType::Stash {
                    ui.show_toast("Deleted stash");
                } else {
                    ui.show_toast(&format!("Deleted {what} “{name}”"));
                }
            }
            Err(e) => ui.show_toast(&format!("Couldn't delete {what}: {}", e.message())),
        }
    });

    dialog.present(Some(window));
}

fn ref_type_label(ref_type: RefType) -> &'static str {
    match ref_type {
        RefType::Branch => "branch",
        RefType::Remote => "remote branch",
        RefType::Tag => "tag",
        RefType::Stash => "stash",
    }
}

fn short_sha(sha: &str) -> &str {
    sha.get(..7).unwrap_or(sha)
}
