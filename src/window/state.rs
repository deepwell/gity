use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use tokio::runtime::Runtime;

#[derive(Clone)]
pub struct AppState {
    pub current_path: Rc<RefCell<Option<PathBuf>>>,
    pub current_branch: Rc<RefCell<Option<String>>>,
    pub file_portal_active: Rc<RefCell<bool>>,
    pub tokio_runtime: Arc<Runtime>,
}

impl AppState {
    pub fn new() -> Self {
        // Create a shared Tokio runtime for file portal operations
        // This ensures DBus connections are properly managed and reused
        let runtime = Runtime::new().expect("Failed to create Tokio runtime for file portal");

        Self {
            current_path: Rc::new(RefCell::new(None)),
            current_branch: Rc::new(RefCell::new(None)),
            file_portal_active: Rc::new(RefCell::new(false)),
            tokio_runtime: Arc::new(runtime),
        }
    }

    pub fn clear_repo(&self) {
        *self.current_path.borrow_mut() = None;
        *self.current_branch.borrow_mut() = None;
    }

    pub fn is_repo_loaded(&self) -> bool {
        self.current_path.borrow().is_some()
    }
}
