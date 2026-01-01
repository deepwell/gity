use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

#[derive(Clone)]
pub struct AppState {
    pub current_path: Rc<RefCell<Option<PathBuf>>>,
    pub current_branch: Rc<RefCell<Option<String>>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            current_path: Rc::new(RefCell::new(None)),
            current_branch: Rc::new(RefCell::new(None)),
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
