use chrono::prelude::*;
use git2::{Commit, DiffOptions, ObjectType, Repository, Signature, Time};
use git2::{DiffFormat, Error, Pathspec};
use std::fmt;
use std::path::Path;
use std::path::PathBuf;
use std::str;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::logger::Logger;

/// Determines which ref the UI should open by default for the repository at `path`.
///
/// Policy:
/// - Prefer the currently checked-out branch (HEAD) if it exists and can be resolved.
/// - Otherwise fall back to `main` if it exists.
/// - Otherwise fall back to `HEAD` (detached/unborn/etc).
///
/// Returned string is suitable for `repo.revparse_single(...)` / `repo.revparse(...)`.
pub fn default_branch_ref(path: &Path) -> String {
    let Ok(repo) = Repository::open(path) else {
        return "HEAD".to_string();
    };

    let resolve = |name: &str| repo.revparse_single(name).is_ok();

    let head = repo.head().ok();
    let head_branch_name = head
        .as_ref()
        .and_then(|h| if h.is_branch() { h.shorthand() } else { None })
        .map(|s| s.to_string());

    if let Some(current) = head_branch_name.as_deref() {
        if resolve(current) {
            return current.to_string();
        }
    }

    if resolve("main") {
        return "main".to_string();
    }

    "HEAD".to_string()
}

pub struct GitCommit {
    pub id: String,
    pub author: String,
    pub message: String,
    pub date: String,
}

impl fmt::Debug for GitCommit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitCommit")
            .field("id", &self.id)
            .field("message", &self.message)
            .field("author", &self.author)
            .field("date", &self.date)
            .finish()
    }
}

/// Commit revwalk ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CommitSort {
    /// Default libgit2 order (typically reverse chronological on the selected ref).
    #[default]
    None,
    /// Topological order.
    Topological,
    /// Commit time order.
    Time,
}

/// Explicit options for listing commits from a repository.
#[derive(Debug, Clone)]
pub struct CommitQueryOptions {
    /// One or more revspecs to push/hide (e.g. `["main"]`, `["HEAD"]`, `["^deadbeef", "main"]`).
    pub revspecs: Vec<String>,
    /// Optional pathspec filters (only include commits touching these paths).
    pub pathspecs: Vec<String>,
    pub sort: CommitSort,
    pub reverse: bool,
    pub author_contains: Option<String>,
    pub committer_contains: Option<String>,
    pub message_contains: Option<String>,
    /// Minimum number of parents required (inclusive).
    pub min_parents: usize,
    /// Maximum number of parents allowed. NOTE: kept as "exclusive" to preserve existing behavior.
    /// (Old code filtered out commits where `parents >= max_parents_exclusive`.)
    pub max_parents_exclusive: Option<usize>,
}

impl CommitQueryOptions {
    pub fn for_branch(branch_ref: &str) -> Self {
        Self {
            revspecs: vec![branch_ref.to_string()],
            pathspecs: Vec::new(),
            sort: CommitSort::None,
            reverse: false,
            author_contains: None,
            committer_contains: None,
            message_contains: None,
            min_parents: 0,
            max_parents_exclusive: None,
        }
    }
}

/// Shared commit-walking core used by both the UI commit list pager and search.
///
/// The walker owns the `Repository` and `Revwalk`, so it can efficiently produce pages
/// without re-initializing the revwalk each time.
pub struct CommitWalker<'repo> {
    repo: &'repo Repository,
    revwalk: git2::Revwalk<'repo>,
    opts: CommitQueryOptions,
    pathspec: Option<Pathspec>,
    diffopts: DiffOptions,
}

impl<'repo> CommitWalker<'repo> {
    pub fn new(repo: &'repo Repository, opts: CommitQueryOptions) -> Result<Self, Error> {
        let mut revwalk = repo.revwalk()?;

        // Prepare the revwalk based on options
        let base = if opts.reverse {
            git2::Sort::REVERSE
        } else {
            git2::Sort::NONE
        };
        revwalk.set_sorting(
            base | if opts.sort == CommitSort::Topological {
                git2::Sort::TOPOLOGICAL
            } else if opts.sort == CommitSort::Time {
                git2::Sort::TIME
            } else {
                git2::Sort::NONE
            },
        )?;

        for spec in &opts.revspecs {
            if spec.starts_with('^') {
                let obj = repo.revparse_single(&spec[1..])?;
                revwalk.hide(obj.id())?;
                continue;
            }
            let revspec = repo.revparse(spec)?;
            if revspec.mode().contains(git2::RevparseMode::SINGLE) {
                revwalk.push(revspec.from().unwrap().id())?;
            } else {
                let from = revspec.from().unwrap().id();
                let to = revspec.to().unwrap().id();
                revwalk.push(to)?;
                if revspec.mode().contains(git2::RevparseMode::MERGE_BASE) {
                    let base = repo.merge_base(from, to)?;
                    let o = repo.find_object(base, Some(ObjectType::Commit))?;
                    revwalk.push(o.id())?;
                }
                revwalk.hide(from)?;
            }
        }
        if opts.revspecs.is_empty() {
            revwalk.push_head()?;
        }

        let mut diffopts = DiffOptions::new();
        if !opts.pathspecs.is_empty() {
            for spec in &opts.pathspecs {
                diffopts.pathspec(spec);
            }
        }
        let pathspec = if opts.pathspecs.is_empty() {
            None
        } else {
            Some(Pathspec::new(opts.pathspecs.iter())?)
        };

        Ok(Self {
            repo,
            revwalk,
            opts,
            pathspec,
            diffopts,
        })
    }

    fn commit_passes_filters(&mut self, commit: &Commit) -> Result<bool, Error> {
        let parents = commit.parents().len();
        if parents < self.opts.min_parents {
            return Ok(false);
        }
        if let Some(n) = self.opts.max_parents_exclusive {
            if parents >= n {
                return Ok(false);
            }
        }

        if let Some(ps) = &self.pathspec {
            match commit.parents().len() {
                0 => {
                    let tree = commit.tree()?;
                    let flags = git2::PathspecFlags::NO_MATCH_ERROR;
                    if ps.match_tree(&tree, flags).is_err() {
                        return Ok(false);
                    }
                }
                _ => {
                    let matches = commit.parents().all(|parent| {
                        match_with_parent(self.repo, commit, &parent, &mut self.diffopts)
                            .unwrap_or(false)
                    });
                    if !matches {
                        return Ok(false);
                    }
                }
            }
        }

        if !sig_matches(&commit.author(), self.opts.author_contains.as_deref()) {
            return Ok(false);
        }
        if !sig_matches(&commit.committer(), self.opts.committer_contains.as_deref()) {
            return Ok(false);
        }
        if !log_message_matches(commit.message(), self.opts.message_contains.as_deref()) {
            return Ok(false);
        }

        Ok(true)
    }

    fn to_git_commit(commit: &Commit) -> GitCommit {
        GitCommit {
            message: String::from_utf8_lossy(commit.message_bytes()).to_string(),
            author: commit.author().name().unwrap_or("").to_string(),
            date: format_datetime(&commit.author().when()),
            id: commit.id().to_string(),
        }
    }

    /// Returns the next matching commit, or `None` if the revwalk is exhausted.
    pub fn next(&mut self, cancel: Option<&Arc<AtomicBool>>) -> Option<Result<GitCommit, Error>> {
        loop {
            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Some(Err(Error::from_str("Cancelled")));
            }

            let oid = match self.revwalk.next()? {
                Ok(oid) => oid,
                Err(e) => return Some(Err(e)),
            };

            if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                return Some(Err(Error::from_str("Cancelled")));
            }

            let commit = match self.repo.find_commit(oid) {
                Ok(c) => c,
                Err(e) => return Some(Err(e)),
            };

            match self.commit_passes_filters(&commit) {
                Ok(true) => return Some(Ok(Self::to_git_commit(&commit))),
                Ok(false) => continue,
                Err(e) => return Some(Err(e)),
            }
        }
    }

    /// Return up to `page_size` commits plus a `done` flag (true iff no more commits exist).
    pub fn next_page(
        &mut self,
        page_size: usize,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> Result<(Vec<GitCommit>, bool), Error> {
        let mut out = Vec::with_capacity(page_size);
        for _ in 0..page_size {
            match self.next(cancel) {
                Some(Ok(c)) => out.push(c),
                Some(Err(e)) => return Err(e),
                None => return Ok((out, true)),
            }
        }
        Ok((out, false))
    }
}

fn sig_matches(sig: &Signature, contains: Option<&str>) -> bool {
    let Some(s) = contains else { return true };
    sig.name().map(|n| n.contains(s)).unwrap_or(false)
        || sig.email().map(|n| n.contains(s)).unwrap_or(false)
}

fn log_message_matches(msg: Option<&str>, contains: Option<&str>) -> bool {
    match (contains, msg) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(s), Some(msg)) => msg.contains(s),
    }
}

fn match_with_parent(
    repo: &Repository,
    commit: &Commit,
    parent: &Commit,
    opts: &mut DiffOptions,
) -> Result<bool, Error> {
    let a = parent.tree()?;
    let b = commit.tree()?;
    let diff = repo.diff_tree_to_tree(Some(&a), Some(&b), Some(opts))?;
    Ok(diff.deltas().len() > 0)
}

fn format_datetime(time: &Time) -> String {
    let dt = Utc
        .timestamp_opt(time.seconds() + (time.offset_minutes() as i64) * 60, 0)
        .unwrap();
    dt.format("%b %d, %Y %H:%M").to_string()
}

#[derive(Clone)]
pub struct BranchInfo {
    pub name: String,
    pub latest_commit_time: DateTime<Utc>,
}

pub fn get_local_branches(path: &str) -> Result<Vec<BranchInfo>, Error> {
    let repo = Repository::open(path)?;
    let branches = repo.branches(Some(git2::BranchType::Local))?;

    let mut branch_infos = Vec::new();
    for branch in branches {
        let (branch, _) = branch?;
        if let Some(name) = branch.name()? {
            // Get latest commit time for this branch
            if let Ok(commit) = branch.get().peel_to_commit() {
                let time = commit.time();
                let commit_time = Utc
                    .timestamp_opt(time.seconds() + (time.offset_minutes() as i64) * 60, 0)
                    .unwrap();

                branch_infos.push(BranchInfo {
                    name: name.to_string(),
                    latest_commit_time: commit_time,
                });
            }
        }
    }

    Ok(branch_infos)
}

#[derive(Clone)]
pub struct TagInfo {
    pub name: String,
    pub commit_time: DateTime<Utc>,
}

/// Returns a list of all tags in the repository with their commit times.
///
/// Tags are returned with the tag name and the time of the commit they point to.
/// For annotated tags, this is the time of the tagged commit (not the tag creation time).
pub fn get_tag_list(path: &Path) -> Result<Vec<TagInfo>, Error> {
    let repo = Repository::open(path)?;
    let mut tag_infos = Vec::new();

    repo.tag_foreach(|oid, name_bytes| {
        // Tag names come as "refs/tags/tagname" - extract just the tag name
        let name = str::from_utf8(name_bytes)
            .ok()
            .and_then(|s| s.strip_prefix("refs/tags/"))
            .unwrap_or_else(|| str::from_utf8(name_bytes).unwrap_or(""))
            .to_string();

        if name.is_empty() {
            return true; // continue iteration
        }

        // Resolve the tag to its target commit and get commit time
        if let Ok(obj) = repo.find_object(oid, None) {
            if let Ok(commit) = obj.peel_to_commit() {
                let time = commit.time();
                let commit_time = Utc
                    .timestamp_opt(time.seconds() + (time.offset_minutes() as i64) * 60, 0)
                    .unwrap();

                tag_infos.push(TagInfo { name, commit_time });
            }
        }

        true // continue iteration
    })?;

    Ok(tag_infos)
}

pub struct CommitMetadata {
    pub author_name: String,
    pub author_email: String,
    pub date_time: String,
    pub commit_message: String,
    pub git_sha: String,
}

pub fn get_commit_metadata(path: &str, commit_sha: &str) -> Result<CommitMetadata, Error> {
    let repo = Repository::open(path)?;
    let commit_oid = git2::Oid::from_str(commit_sha)?;
    let commit = repo.find_commit(commit_oid)?;

    let author = commit.author();
    let author_name = author.name().unwrap_or("").to_string();
    let author_email = author.email().unwrap_or("").to_string();
    let date_time = format_datetime(&author.when());
    let commit_message = String::from_utf8_lossy(commit.message_bytes()).to_string();
    let git_sha = commit_sha.to_string();

    Ok(CommitMetadata {
        author_name,
        author_email,
        date_time,
        commit_message,
        git_sha,
    })
}

pub fn get_commit_diff(path: &str, commit_sha: &str) -> Result<String, Error> {
    let repo = Repository::open(path)?;
    let commit_oid = git2::Oid::from_str(commit_sha)?;
    let commit = repo.find_commit(commit_oid)?;

    let mut diff_text = String::new();

    // Handle merge commits (multiple parents)
    if commit.parents().len() > 1 {
        diff_text.push_str("Merge commit - showing diff against first parent\n\n");
    }

    // Get parent tree if it exists
    let parent_tree = if commit.parents().len() >= 1 {
        let parent = commit.parent(0)?;
        Some(parent.tree()?)
    } else {
        None
    };

    // Get commit tree
    let commit_tree = commit.tree()?;

    // Create diff with a small amount of context around changes (like `git show -U3`).
    let mut diff_opts = DiffOptions::new();
    diff_opts.context_lines(3);
    diff_opts.interhunk_lines(0);
    let diff = repo.diff_tree_to_tree(
        parent_tree.as_ref(),
        Some(&commit_tree),
        Some(&mut diff_opts),
    )?;

    // Format diff as patch
    diff.print(DiffFormat::Patch, |_delta, _hunk, line| {
        match line.origin() {
            ' ' | '+' | '-' => {
                diff_text.push(line.origin());
            }
            _ => {}
        }
        if let Ok(content) = str::from_utf8(line.content()) {
            diff_text.push_str(content);
        }
        true
    })?;

    Ok(diff_text)
}

pub fn validate_repository(path: &Path) -> Result<(), git2::Error> {
    Repository::open(path).map(|_| ())
}

/// If `path` is inside a git worktree (or is a bare repo), return the best path
/// to open in the UI:
/// - worktree root for normal repositories
/// - repo directory for bare repositories
pub fn discover_repository_root(path: &Path) -> Option<PathBuf> {
    let repo = Repository::discover(path).ok()?;
    if let Some(workdir) = repo.workdir() {
        Some(workdir.to_path_buf())
    } else {
        // Bare repository
        Some(repo.path().to_path_buf())
    }
}

pub fn branch_exists(path: &Path, branch_name: &str) -> bool {
    let Ok(repo) = Repository::open(path) else {
        return false;
    };
    repo.revparse_single(branch_name).is_ok()
}

/// Returns the currently checked-out branch name for the repository at `path`.
/// If HEAD is detached (or can't be read), returns `None`.
pub fn checked_out_branch_name(path: &Path) -> Option<String> {
    let repo = Repository::open(path).ok()?;
    let head = repo.head().ok()?;
    if !head.is_branch() {
        return None;
    }
    head.shorthand().map(|s| s.to_string())
}

/// Returns a mapping from commit SHA to tag names for all tags in the repository.
///
/// The returned HashMap maps full commit SHA strings to vectors of tag names
/// that point to that commit (either directly or through tag objects).
pub fn get_tags(path: &Path) -> Result<std::collections::HashMap<String, Vec<String>>, Error> {
    use std::collections::HashMap;

    let repo = Repository::open(path)?;
    let mut tag_map: HashMap<String, Vec<String>> = HashMap::new();

    repo.tag_foreach(|oid, name_bytes| {
        // Tag names come as "refs/tags/tagname" - extract just the tag name
        let name = str::from_utf8(name_bytes)
            .ok()
            .and_then(|s| s.strip_prefix("refs/tags/"))
            .unwrap_or_else(|| str::from_utf8(name_bytes).unwrap_or(""))
            .to_string();

        if name.is_empty() {
            return true; // continue iteration
        }

        // Resolve the tag to its target commit
        // For lightweight tags, oid is the commit directly
        // For annotated tags, we need to peel to the commit
        let commit_oid = if let Ok(obj) = repo.find_object(oid, None) {
            if let Ok(commit) = obj.peel_to_commit() {
                commit.id()
            } else {
                oid
            }
        } else {
            oid
        };

        tag_map
            .entry(commit_oid.to_string())
            .or_default()
            .push(name);

        true // continue iteration
    })?;

    // Sort tags alphabetically within each commit for consistent display
    for tags in tag_map.values_mut() {
        tags.sort();
    }

    let tag_count: usize = tag_map.values().map(|v| v.len()).sum();
    Logger::info(&format!("Found {} tags in ({})", tag_count, path.display()));

    Ok(tag_map)
}
