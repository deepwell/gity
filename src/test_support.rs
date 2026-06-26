//! Test-only helpers for building deterministic git repositories in a tempdir.
//!
//! These helpers use `git2` directly to construct known commit graphs so the
//! `git` and `search` modules can be exercised against real repositories.
//!
//! Some helpers are reserved for upcoming branch/tag/diff tests, so unused
//! methods are tolerated here.
#![allow(dead_code)]

use git2::{Commit, Oid, Repository, Signature, Time};
use std::path::Path;
use tempfile::TempDir;

/// A throwaway git repository living in a `TempDir`.
///
/// Commits are authored with a monotonically increasing timestamp (one minute
/// apart) so revwalk ordering is deterministic across runs.
pub struct TestRepo {
    dir: TempDir,
    repo: Repository,
    clock: i64,
    seq: u32,
}

impl TestRepo {
    /// Initialize an empty repository in a fresh tempdir.
    pub fn new() -> Self {
        let dir = tempfile::tempdir().expect("create tempdir");
        let repo = Repository::init(dir.path()).expect("git init");
        Self {
            dir,
            repo,
            clock: 1_700_000_000,
            seq: 0,
        }
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    pub fn repo(&self) -> &Repository {
        &self.repo
    }

    fn next_time(&mut self) -> Time {
        let t = self.clock;
        self.clock += 60;
        Time::new(t, 0)
    }

    fn next_seq(&mut self) -> u32 {
        let s = self.seq;
        self.seq += 1;
        s
    }

    /// Commit `message` on `main`, writing a unique file so the tree changes.
    pub fn commit(&mut self, message: &str) -> Oid {
        self.commit_on("main", message)
    }

    /// Commit `message` on `branch` (created if it does not yet exist).
    pub fn commit_on(&mut self, branch: &str, message: &str) -> Oid {
        let seq = self.next_seq();
        self.commit_detailed(
            branch,
            &format!("file_{seq}.txt"),
            &format!("contents {seq}\n"),
            message,
            "Tester",
            "tester@example.com",
            "Tester",
            "tester@example.com",
        )
    }

    /// Commit with a specific author (committer mirrors the author).
    pub fn commit_by(
        &mut self,
        branch: &str,
        message: &str,
        author_name: &str,
        author_email: &str,
    ) -> Oid {
        let seq = self.next_seq();
        self.commit_detailed(
            branch,
            &format!("file_{seq}.txt"),
            &format!("contents {seq}\n"),
            message,
            author_name,
            author_email,
            author_name,
            author_email,
        )
    }

    /// Append a line to a specific file path and commit it on `branch`.
    pub fn commit_file(&mut self, branch: &str, file: &str, contents: &str, message: &str) -> Oid {
        self.commit_detailed(
            branch,
            file,
            contents,
            message,
            "Tester",
            "tester@example.com",
            "Tester",
            "tester@example.com",
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit_detailed(
        &mut self,
        branch: &str,
        file: &str,
        contents: &str,
        message: &str,
        author_name: &str,
        author_email: &str,
        committer_name: &str,
        committer_email: &str,
    ) -> Oid {
        let when = self.next_time();
        let ref_name = format!("refs/heads/{branch}");
        let parent_commit = self
            .repo
            .find_reference(&ref_name)
            .ok()
            .and_then(|r| r.peel_to_commit().ok());

        let tree_oid = {
            let mut index = self.repo.index().expect("open index");
            match &parent_commit {
                Some(pc) => index.read_tree(&pc.tree().unwrap()).expect("seed index"),
                None => index.clear().expect("clear index"),
            }

            let workdir = self.repo.workdir().expect("workdir");
            let full = workdir.join(file);
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).expect("mkdir");
            }
            std::fs::write(&full, contents).expect("write file");

            index.add_path(Path::new(file)).expect("add path");
            let oid = index.write_tree().expect("write tree");
            index.write().expect("persist index");
            oid
        };

        let tree = self.repo.find_tree(tree_oid).expect("find tree");
        let author = Signature::new(author_name, author_email, &when).expect("author sig");
        let committer =
            Signature::new(committer_name, committer_email, &when).expect("committer sig");

        let parents: Vec<&Commit> = parent_commit.iter().collect();
        self.repo
            .commit(
                Some(&ref_name),
                &author,
                &committer,
                message,
                &tree,
                &parents,
            )
            .expect("create commit")
    }

    /// Create a merge commit on `branch` with a second parent from `other_branch`.
    /// The resulting tree mirrors `branch`'s current tip (no real merge performed).
    pub fn merge_commit(&mut self, branch: &str, other_branch: &str, message: &str) -> Oid {
        let when = self.next_time();
        let c1 = self.repo.find_commit(self.tip(branch)).unwrap();
        let c2 = self.repo.find_commit(self.tip(other_branch)).unwrap();
        let tree = c1.tree().unwrap();
        let sig = Signature::new("Tester", "tester@example.com", &when).unwrap();
        self.repo
            .commit(
                Some(&format!("refs/heads/{branch}")),
                &sig,
                &sig,
                message,
                &tree,
                &[&c1, &c2],
            )
            .expect("create merge commit")
    }

    /// Branch tip commit id.
    pub fn tip(&self, branch: &str) -> Oid {
        self.repo
            .find_reference(&format!("refs/heads/{branch}"))
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
    }

    /// Create `name` pointing at the tip of `from`.
    pub fn create_branch(&self, name: &str, from: &str) {
        let target = self
            .repo
            .find_reference(&format!("refs/heads/{from}"))
            .unwrap()
            .peel_to_commit()
            .unwrap();
        self.repo
            .branch(name, &target, false)
            .expect("create branch");
    }

    /// Point HEAD at `branch` (does not touch the working tree).
    pub fn checkout(&self, branch: &str) {
        self.repo
            .set_head(&format!("refs/heads/{branch}"))
            .expect("set head");
    }

    pub fn lightweight_tag(&self, name: &str, target: Oid) {
        let obj = self.repo.find_object(target, None).unwrap();
        self.repo
            .tag_lightweight(name, &obj, false)
            .expect("lightweight tag");
    }

    pub fn annotated_tag(&mut self, name: &str, target: Oid, message: &str) {
        let when = self.next_time();
        let obj = self.repo.find_object(target, None).unwrap();
        let tagger = Signature::new("Tester", "tester@example.com", &when).unwrap();
        self.repo
            .tag(name, &obj, &tagger, message, false)
            .expect("annotated tag");
    }

    /// Create a remote-tracking ref, e.g. `name = "origin/main"`.
    pub fn create_remote_ref(&self, name: &str, target: Oid) {
        self.repo
            .reference(&format!("refs/remotes/{name}"), target, true, "test remote")
            .expect("create remote ref");
    }
}
