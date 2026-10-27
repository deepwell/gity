use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use tokio::runtime::Runtime;

use crate::ui::RefType;

type RepoListener = Rc<dyn Fn()>;

#[derive(Clone)]
pub struct AppState {
    pub current_path: Rc<RefCell<Option<PathBuf>>>,
    /// The name of the currently viewed ref (branch or tag)
    pub current_ref: Rc<RefCell<Option<String>>>,
    /// The type of the currently viewed ref
    pub current_ref_type: Rc<RefCell<Option<RefType>>>,
    pub file_portal_active: Rc<RefCell<bool>>,
    pub tokio_runtime: Arc<Runtime>,
    /// `git::repo_state_fingerprint` of the repository as last loaded.
    pub loaded_fingerprint: Rc<RefCell<Option<u64>>>,
    /// Called whenever a repository is loaded or closed.
    repo_listener: Rc<RefCell<Option<RepoListener>>>,
}

impl AppState {
    pub fn new() -> Self {
        // Create a shared Tokio runtime for file portal operations
        // This ensures DBus connections are properly managed and reused
        let runtime = Runtime::new().expect("Failed to create Tokio runtime for file portal");

        Self {
            current_path: Rc::new(RefCell::new(None)),
            current_ref: Rc::new(RefCell::new(None)),
            current_ref_type: Rc::new(RefCell::new(None)),
            file_portal_active: Rc::new(RefCell::new(false)),
            tokio_runtime: Arc::new(runtime),
            loaded_fingerprint: Rc::new(RefCell::new(None)),
            repo_listener: Rc::new(RefCell::new(None)),
        }
    }

    pub fn clear_repo(&self) {
        *self.current_path.borrow_mut() = None;
        *self.current_ref.borrow_mut() = None;
        *self.current_ref_type.borrow_mut() = None;
        *self.loaded_fingerprint.borrow_mut() = None;
        self.notify_repo_changed();
    }

    pub fn set_repo_listener(&self, listener: Option<RepoListener>) {
        *self.repo_listener.borrow_mut() = listener;
    }

    /// Tell the listener a repository was loaded or closed.
    pub fn notify_repo_changed(&self) {
        // Clone out first so the listener may replace itself.
        let listener = self.repo_listener.borrow().clone();
        if let Some(listener) = listener {
            listener();
        }
    }

    pub fn is_repo_loaded(&self) -> bool {
        self.current_path.borrow().is_some()
    }
}
