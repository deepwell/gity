use gtk::gio;
use gtk::prelude::SettingsExt;
use std::path::PathBuf;

use crate::APP_ID;
use crate::git;

use serde::{Deserialize, Serialize};

const MAX_RECENT_REPOS: usize = 12;

/// Stored repository entry with both real and sandbox paths
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RecentRepoEntry {
    real_path: String,
    sandbox_path: String,
}

/// Information about a recent repository for display
pub struct RecentRepo {
    /// The sandbox path (used for actual operations)
    pub path: PathBuf,
    /// The real path (used for display)
    pub real_path: PathBuf,
    pub folder_name: String,
    pub display_path: String,
}

/// Format a path for display, replacing the home directory with ~/
fn format_display_path(path: &PathBuf) -> String {
    let path_str = path.to_string_lossy();
    if let Some(home_dir) = dirs::home_dir() {
        let home_str = home_dir.to_string_lossy();
        if path_str.starts_with(home_str.as_ref()) {
            return format!("~{}", &path_str[home_str.len()..]);
        }
    }
    path_str.to_string()
}

/// Load recent repositories from GSettings
pub fn load_recent_repos() -> Vec<RecentRepo> {
    let settings = gio::Settings::new(APP_ID);
    let json_str = settings.string("recent-repositories");

    let entries: Vec<RecentRepoEntry> =
        serde_json::from_str(&json_str).unwrap_or_else(|_| Vec::new());

    entries
        .into_iter()
        .filter_map(|entry| {
            let sandbox_path = PathBuf::from(&entry.sandbox_path);
            let real_path = PathBuf::from(&entry.real_path);

            // Only include repos that still exist and are valid (check sandbox path for access)
            if sandbox_path.exists() && git::validate_repository(&sandbox_path).is_ok() {
                let folder_name = real_path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&entry.real_path)
                    .to_string();
                let display_path = format_display_path(&real_path);
                Some(RecentRepo {
                    path: sandbox_path,
                    real_path,
                    folder_name,
                    display_path,
                })
            } else {
                None
            }
        })
        .take(MAX_RECENT_REPOS)
        .collect()
}

/// Add a repository path to the recent list (at the front)
///
/// # Arguments
/// * `sandbox_path` - The path accessible in the sandbox (used for operations)
/// * `real_path` - The real path (used for display)
pub fn add_recent_repo(sandbox_path: &PathBuf, real_path: &PathBuf) {
    let settings = gio::Settings::new(APP_ID);
    let json_str = settings.string("recent-repositories");

    let mut entries: Vec<RecentRepoEntry> =
        serde_json::from_str(&json_str).unwrap_or_else(|_| Vec::new());

    let sandbox_str = sandbox_path.to_string_lossy().to_string();
    let real_str = real_path.to_string_lossy().to_string();

    // Remove this entry if it exists (to move it to front)
    entries.retain(|e| e.sandbox_path != sandbox_str);

    // Add to front
    entries.insert(
        0,
        RecentRepoEntry {
            real_path: real_str,
            sandbox_path: sandbox_str,
        },
    );

    // Limit to max
    entries.truncate(MAX_RECENT_REPOS);

    // Save
    if let Ok(json) = serde_json::to_string(&entries) {
        let _ = settings.set_string("recent-repositories", &json);
    }
}

/// Remove a repository path from the recent list
///
/// # Arguments
/// * `path` - The sandbox path to remove (matches against sandbox_path in entries)
pub fn remove_recent_repo(path: &PathBuf) {
    let settings = gio::Settings::new(APP_ID);
    let json_str = settings.string("recent-repositories");

    let mut entries: Vec<RecentRepoEntry> =
        serde_json::from_str(&json_str).unwrap_or_else(|_| Vec::new());

    let path_str = path.to_string_lossy().to_string();

    // Filter out the path to remove
    entries.retain(|e| e.sandbox_path != path_str);

    // Save
    if let Ok(json) = serde_json::to_string(&entries) {
        let _ = settings.set_string("recent-repositories", &json);
    }
}
