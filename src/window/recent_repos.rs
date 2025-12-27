use gtk::gio;
use gtk::prelude::SettingsExtManual;
use std::path::PathBuf;

use crate::APP_ID;
use crate::git;

const MAX_RECENT_REPOS: usize = 12;

/// Information about a recent repository for display
pub struct RecentRepo {
    pub path: PathBuf,
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
    let paths: Vec<String> = settings
        .strv("recent-repositories")
        .iter()
        .map(|s| s.to_string())
        .collect();

    paths
        .into_iter()
        .filter_map(|path_str| {
            let path = PathBuf::from(&path_str);
            // Only include repos that still exist and are valid
            if path.exists() && git::validate_repository(&path).is_ok() {
                let folder_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(&path_str)
                    .to_string();
                let display_path = format_display_path(&path);
                Some(RecentRepo {
                    path,
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

/// Normalize a path string by removing trailing slashes
fn normalize_path(path_str: &str) -> String {
    path_str.trim_end_matches('/').to_string()
}

/// Add a repository path to the recent list (at the front)
pub fn add_recent_repo(path: &PathBuf) {
    let settings = gio::Settings::new(APP_ID);
    let path_str = normalize_path(&path.to_string_lossy());

    // Get current list, normalize paths, and remove this path if it exists (to move it to front)
    let mut paths: Vec<String> = settings
        .strv("recent-repositories")
        .iter()
        .map(|s| normalize_path(&s))
        .filter(|p| p != &path_str)
        .collect();

    // Add to front
    paths.insert(0, path_str);

    // Limit to max
    paths.truncate(MAX_RECENT_REPOS);

    // Save
    let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    let _ = settings.set_strv("recent-repositories", path_refs);
}

/// Remove a repository path from the recent list
pub fn remove_recent_repo(path: &PathBuf) {
    let settings = gio::Settings::new(APP_ID);
    let path_str = normalize_path(&path.to_string_lossy());

    // Get current list and filter out the path to remove
    let paths: Vec<String> = settings
        .strv("recent-repositories")
        .iter()
        .map(|s| normalize_path(&s))
        .filter(|p| p != &path_str)
        .collect();

    // Save
    let path_refs: Vec<&str> = paths.iter().map(|s| s.as_str()).collect();
    let _ = settings.set_strv("recent-repositories", path_refs);
}
