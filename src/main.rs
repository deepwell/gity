mod git;
mod logger;
mod search;
mod ui;
mod window;

use adw;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

pub const APP_ID: &str = "com.markdeepwell.GitY";
pub const DEVELOPER_NAME: &str = "Mark Deepwell";

fn main() -> glib::ExitCode {
    // Handle simple CLI flags before initializing GTK / GIO.
    // We intentionally avoid full argument parsing so we don't interfere with
    // `gio::Application` / GTK's built-in `--gapplication-*` flags.
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--version" || arg == "-v")
    {
        println!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
        return glib::ExitCode::SUCCESS;
    }

    gio::resources_register_include!("resources.gresource").expect("Failed to register resources.");

    // Set GSETTINGS_SCHEMA_DIR for development builds if not already set
    // This must be done before any Settings objects are created
    // Safe to use unsafe here because we're in main() before any threads are spawned
    if std::env::var("GSETTINGS_SCHEMA_DIR").is_err() {
        let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
        let schema_dir = std::path::Path::new("target")
            .join(&profile)
            .join("schemas");
        if schema_dir.exists() {
            // Convert to absolute path
            if let Ok(absolute_schema_dir) = std::env::current_dir()
                .map(|cwd| cwd.join(&schema_dir).canonicalize().unwrap_or(schema_dir))
            {
                unsafe {
                    std::env::set_var(
                        "GSETTINGS_SCHEMA_DIR",
                        absolute_schema_dir.to_string_lossy().as_ref(),
                    );
                }
            }
        }
    }

    // Parse command-line arguments for repository path
    let repo_arg: Option<std::path::PathBuf> = std::env::args_os().skip(1).find_map(|a| {
        // Ignore flags; treat the first non-flag arg as a path
        let s = a.to_string_lossy();
        if s.starts_with('-') {
            None
        } else {
            Some(std::path::PathBuf::from(a))
        }
    });

    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_startup(|app| {
        let css_provider = gtk::CssProvider::new();
        css_provider.load_from_resource("/com/markdeepwell/gity/style.css");
        gtk::style_context_add_provider_for_display(
            &gdk::Display::default().expect("Could not get default display"),
            &css_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        window::setup_shortcuts(app);
        window::setup_app_action(app);
    });

    let repo_arg_for_activate = repo_arg.clone();
    app.connect_activate(move |app| {
        // Only load from command-line args on the first window (initial launch)
        let is_first_window = app.windows().is_empty();
        window::build_ui(app, if is_first_window { repo_arg_for_activate.as_ref() } else { None });
    });

    app.run()
}
