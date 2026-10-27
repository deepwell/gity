//! Reload the open repository when its refs change on disk (the "auto-refresh"
//! setting).
//!
//! Changes are noticed with inotify where it works, or by polling for
//! repositories opened through the document portal, whose FUSE mount does not
//! report changes made outside the sandbox. Either way, a reload only happens
//! when `git::repo_state_fingerprint` differs from what was last loaded, so
//! the app's own ref changes and unrelated file churn are ignored.

use gtk::{gio, glib, prelude::*};
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use crate::git;
use crate::logger::Logger;

use super::repo;
use super::state::AppState;
use super::ui::WindowUi;

const SETTING: &str = "auto-refresh";
const POLL_INTERVAL_SECS: u32 = 2;
/// Quiet period after the last file event, so a multi-step git operation
/// (rebase, fetch) triggers one reload at the end.
const DEBOUNCE: Duration = Duration::from_secs(1);

enum Trigger {
    Poll(glib::SourceId),
    Monitor(Vec<gio::FileMonitor>),
}

struct Watch {
    path: PathBuf,
    trigger: Trigger,
}

#[derive(Default)]
struct Inner {
    watch: Option<Watch>,
    debounce: Option<glib::SourceId>,
    checking: bool,
    /// A check was requested while another was running.
    recheck: bool,
    /// The fingerprint seen by the previous poll, so a reload waits until
    /// the repository stops changing.
    last_poll: Option<u64>,
}

#[derive(Clone)]
struct AutoRefresh {
    ui: WindowUi,
    state: AppState,
    settings: gio::Settings,
    inner: Rc<RefCell<Inner>>,
}

pub fn install(
    window: &gtk::ApplicationWindow,
    ui: &WindowUi,
    state: &AppState,
    settings: &gio::Settings,
) {
    let auto_refresh = AutoRefresh {
        ui: ui.clone(),
        state: state.clone(),
        settings: settings.clone(),
        inner: Rc::new(RefCell::new(Inner::default())),
    };

    let for_listener = auto_refresh.clone();
    state.set_repo_listener(Some(Rc::new(move || for_listener.sync())));

    let for_setting = auto_refresh.clone();
    let setting_handler = settings.connect_changed(Some(SETTING), move |_, _| {
        for_setting.sync();
    });

    // Break the reference cycles through the timer, monitors, listener and
    // settings handler so the window can be freed.
    let state_for_destroy = state.clone();
    let settings_for_destroy = settings.clone();
    let setting_handler = RefCell::new(Some(setting_handler));
    window.connect_destroy(move |_| {
        auto_refresh.stop();
        state_for_destroy.set_repo_listener(None);
        if let Some(handler) = setting_handler.take() {
            settings_for_destroy.disconnect(handler);
        }
    });
}

/// Whether `path` is inside the document portal's FUSE mount
/// (`/run/user/<uid>/doc/…`), where inotify misses changes made on the host.
fn uses_document_portal(path: &Path) -> bool {
    path.starts_with(glib::user_runtime_dir().join("doc"))
}

impl AutoRefresh {
    /// Watch the open repository if the setting is on, else stop watching.
    fn sync(&self) {
        let wanted = if self.settings.boolean(SETTING) {
            self.state.current_path.borrow().clone()
        } else {
            None
        };
        let watching = self.inner.borrow().watch.as_ref().map(|w| w.path.clone());
        if wanted == watching {
            return;
        }

        self.stop();
        if let Some(path) = wanted {
            self.start(path);
        }
    }

    fn start(&self, path: PathBuf) {
        let monitors = if uses_document_portal(&path) {
            None
        } else {
            self.monitor_repo(&path)
        };
        let trigger = match monitors {
            Some(monitors) => {
                Logger::info(&format!("Auto-refresh: watching {}", path.display()));
                Trigger::Monitor(monitors)
            }
            None => {
                Logger::info(&format!("Auto-refresh: polling {}", path.display()));
                let this = self.clone();
                Trigger::Poll(glib::timeout_add_seconds_local(
                    POLL_INTERVAL_SECS,
                    move || {
                        this.check(true);
                        glib::ControlFlow::Continue
                    },
                ))
            }
        };
        self.inner.borrow_mut().watch = Some(Watch { path, trigger });

        // Catch anything that changed while auto-refresh was off.
        self.check(false);
    }

    fn stop(&self) {
        let mut inner = self.inner.borrow_mut();
        if let Some(debounce) = inner.debounce.take() {
            debounce.remove();
        }
        if let Some(watch) = inner.watch.take() {
            match watch.trigger {
                Trigger::Poll(source) => source.remove(),
                Trigger::Monitor(monitors) => {
                    for monitor in monitors {
                        monitor.cancel();
                    }
                }
            }
        }
        inner.recheck = false;
        inner.last_poll = None;
    }

    /// Set up inotify monitors for the files that hold HEAD, refs and the
    /// stash list. Returns `None` if monitoring isn't available.
    fn monitor_repo(&self, path: &Path) -> Option<Vec<gio::FileMonitor>> {
        let (git_dir, common_dir) = git::git_dirs(path)?;
        let mut monitors = Vec::new();

        let mut top_dirs = vec![git_dir.clone()];
        if common_dir != git_dir {
            top_dirs.push(common_dir.clone());
        }
        for dir in top_dirs {
            monitors.push(self.monitor_git_dir(&dir)?);
        }

        let mut trees = vec![common_dir.join("refs")];
        let logs = common_dir.join("logs");
        if logs.is_dir() {
            trees.push(logs);
        }
        for tree in trees {
            self.monitor_tree(&tree, &mut monitors)?;
        }
        Some(monitors)
    }

    /// Watch a git directory itself, reacting only to `HEAD` and
    /// `packed-refs` (it also sees `index`, `ORIG_HEAD` and lock files).
    fn monitor_git_dir(&self, dir: &Path) -> Option<gio::FileMonitor> {
        let monitor = gio::File::for_path(dir)
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
            .ok()?;
        let this = self.clone();
        monitor.connect_changed(move |_, file, other_file, event| {
            let named = |name: &str| {
                [Some(file), other_file]
                    .into_iter()
                    .flatten()
                    .any(|f| f.basename().is_some_and(|b| b == Path::new(name)))
            };
            if named("HEAD") || named("packed-refs") {
                this.schedule_check();
            } else if event == gio::FileMonitorEvent::Created && named("logs") {
                // The first reflog write creates `logs/`.
                this.add_tree_monitor(file);
                this.schedule_check();
            }
        });
        Some(monitor)
    }

    /// Watch `dir` and every directory below it (`refs/heads/feature/…`).
    fn monitor_tree(&self, dir: &Path, monitors: &mut Vec<gio::FileMonitor>) -> Option<()> {
        let file = gio::File::for_path(dir);
        let monitor = file
            .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
            .ok()?;
        let this = self.clone();
        monitor.connect_changed(move |_, file, _, event| {
            if event == gio::FileMonitorEvent::Created {
                this.add_tree_monitor(file);
            }
            this.schedule_check();
        });
        monitors.push(monitor);

        for entry in std::fs::read_dir(dir).ok()?.flatten() {
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                self.monitor_tree(&entry.path(), monitors)?;
            }
        }
        Some(())
    }

    /// Start watching a directory created after monitoring began.
    fn add_tree_monitor(&self, file: &gio::File) {
        let is_dir = file.query_file_type(gio::FileQueryInfoFlags::NONE, gio::Cancellable::NONE)
            == gio::FileType::Directory;
        let Some(path) = file.path().filter(|_| is_dir) else {
            return;
        };
        let mut added = Vec::new();
        let _ = self.monitor_tree(&path, &mut added);
        if let Some(Watch {
            trigger: Trigger::Monitor(monitors),
            ..
        }) = &mut self.inner.borrow_mut().watch
        {
            monitors.extend(added);
        }
    }

    /// Check once file events have been quiet for `DEBOUNCE`.
    fn schedule_check(&self) {
        let mut inner = self.inner.borrow_mut();
        if let Some(debounce) = inner.debounce.take() {
            debounce.remove();
        }
        let this = self.clone();
        inner.debounce = Some(glib::timeout_add_local_once(DEBOUNCE, move || {
            this.inner.borrow_mut().debounce = None;
            this.check(false);
        }));
    }

    /// Fingerprint the repository off the main thread and reload if it
    /// changed. With `settle`, a change must be seen by two polls in a row.
    fn check(&self, settle: bool) {
        let Some(path) = self.state.current_path.borrow().clone() else {
            return;
        };
        {
            let mut inner = self.inner.borrow_mut();
            if inner.checking {
                inner.recheck = true;
                return;
            }
            inner.checking = true;
        }

        let (tx, rx) = mpsc::channel();
        let path_for_thread = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(git::repo_state_fingerprint(&path_for_thread).ok());
        });

        let this = self.clone();
        glib::timeout_add_local(Duration::from_millis(20), move || match rx.try_recv() {
            Ok(fingerprint) => {
                this.finish_check(&path, fingerprint, settle);
                glib::ControlFlow::Break
            }
            Err(mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
            Err(mpsc::TryRecvError::Disconnected) => {
                this.finish_check(&path, None, settle);
                glib::ControlFlow::Break
            }
        });
    }

    fn finish_check(&self, path: &Path, fingerprint: Option<u64>, settle: bool) {
        let (previous, recheck) = {
            let mut inner = self.inner.borrow_mut();
            inner.checking = false;
            let recheck = std::mem::take(&mut inner.recheck);
            let watching = inner.watch.as_ref().is_some_and(|w| w.path == path);
            if !watching || self.state.current_path.borrow().as_deref() != Some(path) {
                // Stopped, or switched repositories, while checking.
                return;
            }
            let previous = std::mem::replace(&mut inner.last_poll, fingerprint);
            (previous, recheck)
        };

        if let Some(fingerprint) = fingerprint {
            let loaded = *self.state.loaded_fingerprint.borrow();
            match loaded {
                Some(loaded) if loaded == fingerprint => {}
                // The load couldn't fingerprint the repo; adopt this as current.
                None => *self.state.loaded_fingerprint.borrow_mut() = Some(fingerprint),
                Some(_) if settle && previous != Some(fingerprint) => {}
                Some(_) => {
                    Logger::info("Auto-refresh: repository changed on disk, reloading");
                    repo::refresh_repo(&self.ui, &self.state, super::APP_NAME);
                }
            }
        }

        if recheck {
            self.check(settle);
        }
    }
}
