/// Logger module that only prints in debug builds
pub struct Logger;

impl Logger {
    /// Log an info message (only prints in debug builds)
    #[cfg(debug_assertions)]
    pub fn info(message: &str) {
        println!("[INFO] {}", message);
    }

    /// Log an info message (no-op in release builds)
    #[cfg(not(debug_assertions))]
    pub fn info(_message: &str) {
        // No-op in release builds
    }

    /// Log an error message (only prints in debug builds)
    #[cfg(debug_assertions)]
    pub fn error(message: &str) {
        eprintln!("{}", message);
    }

    /// Log an error message (no-op in release builds)
    #[cfg(not(debug_assertions))]
    pub fn error(_message: &str) {
        // No-op in release builds
    }
}
