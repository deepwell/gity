mod git;
mod logger;
mod search;
mod ui;
mod version;

#[cfg(test)]
mod test_support;

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
        println!("{} {}", env!("CARGO_PKG_NAME"), version::app_version());
        return glib::ExitCode::SUCCESS;
    }

    gio::resources_register_include!("resources.gresource").expect("Failed to register resources.");

    // Set GSETTINGS_SCHEMA_DIR for development builds if not already set
    // This must be done before any Settings objects are created
    // Release builds always use the installed system schema; otherwise launching
    // the installed binary from the source tree would pick up a stale dev schema.
    // Safe to use unsafe here because we're in main() before any threads are spawned
    #[cfg(debug_assertions)]
    if std::env::var("GSETTINGS_SCHEMA_DIR").is_err() {
        // build.rs compiles schemas relative to the manifest dir, so anchor there
        // instead of the current working directory
        let schema_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("debug")
            .join("schemas");
        if schema_dir.exists() {
            unsafe {
                std::env::set_var("GSETTINGS_SCHEMA_DIR", &schema_dir);
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
        let display = gdk::Display::default().expect("Could not get default display");

        let css_provider = gtk::CssProvider::new();
        css_provider.load_from_resource("/com/markdeepwell/gity/style.css");
        gtk::style_context_add_provider_for_display(
            &display,
            &css_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );

        // Make the bundled application icon available to the icon theme so it
        // resolves by name (e.g. on the welcome screen and the About dialog),
        // including in development runs where system icons aren't installed.
        gtk::IconTheme::for_display(&display).add_resource_path("/com/markdeepwell/gity/icons");

        ui::setup_shortcuts(app);
        ui::setup_app_action(app);
    });

    let repo_arg_for_activate = repo_arg.clone();
    app.connect_activate(move |app| {
        // Only load from command-line args on the first window (initial launch)
        let is_first_window = app.windows().is_empty();
        ui::build_ui(
            app,
            if is_first_window {
                repo_arg_for_activate.as_ref()
            } else {
                None
            },
        );
    });

    app.run()
}
