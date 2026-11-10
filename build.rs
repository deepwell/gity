use std::path::Path;
use std::process::Command;

/// Application ID of release builds, carried by everything under `data/`.
const APP_ID: &str = "com.markdeepwell.GitY";
/// Application ID of development builds.
const DEVEL_APP_ID: &str = "com.markdeepwell.GitY.Devel";

const SCHEMA_FILE: &str = "data/com.markdeepwell.GitY.gschema.xml";
const SETTINGS_PATH: &str = "/com/markdeepwell/gity/";
const DEVEL_SETTINGS_PATH: &str = "/com/markdeepwell/gity-devel/";

fn main() {
    let app_version = std::env::var("MESON_PROJECT_VERSION").unwrap_or_else(|_| {
        std::env::var("CARGO_PKG_VERSION").expect("CARGO_PKG_VERSION must be set for build.rs")
    });
    println!("cargo:rustc-env=GITY_APP_VERSION={app_version}");
    println!("cargo:rerun-if-env-changed=MESON_PROJECT_VERSION");

    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    // A debug build made directly with Cargo (`cargo run`) gets its own
    // application ID, so it runs next to an installed GitY instead of handing
    // off to it, and keeps its own settings. Meson builds keep the release ID
    // whatever their buildtype, because the desktop file, schema and Flatpak
    // manifest they are installed with all carry it.
    let meson_build = std::env::var_os("MESON_PROJECT_VERSION").is_some();
    let app_id = if profile == "debug" && !meson_build {
        DEVEL_APP_ID
    } else {
        APP_ID
    };
    println!("cargo:rustc-env=GITY_APP_ID={app_id}");

    glib_build_tools::compile_resources(
        &["src/resources"],
        "src/resources/resources.gresource.xml",
        "resources.gresource",
    );

    // Tell Cargo to rerun this build script if the schema file changes
    println!("cargo:rerun-if-changed={SCHEMA_FILE}");

    // Compile GSettings schemas
    // For development, compile to both debug and release directories
    let output_dir = Path::new("target").join(&profile);
    let schemas_dir = output_dir.join("schemas");

    let status = stage_schemas(&schemas_dir).and_then(|()| {
        Command::new("glib-compile-schemas")
            .arg(&schemas_dir)
            .status()
    });

    match status {
        Ok(exit_status) if exit_status.success() => {
            println!(
                "cargo:rustc-env=GSETTINGS_SCHEMA_DIR={}",
                schemas_dir.display()
            );
        }
        Ok(exit_status) => {
            eprintln!(
                "Warning: glib-compile-schemas exited with status: {:?}",
                exit_status
            );
            eprintln!("GSettings schema compilation may have failed");
        }
        Err(e) => {
            eprintln!("Warning: Failed to compile GSettings schemas: {}", e);
            eprintln!(
                "Make sure glib-compile-schemas is installed (usually part of glib2-devel or glib2-tools)"
            );
            eprintln!("GSettings functionality will not work without compiled schemas");
        }
    }
}

/// Writes the schemas to compile into `schemas_dir`: the release schema and its
/// development copy, so the directory serves a build with either application ID.
fn stage_schemas(schemas_dir: &Path) -> std::io::Result<()> {
    let schema = std::fs::read_to_string(SCHEMA_FILE)?;
    std::fs::create_dir_all(schemas_dir)?;
    std::fs::write(schemas_dir.join(format!("{APP_ID}.gschema.xml")), &schema)?;
    std::fs::write(
        schemas_dir.join(format!("{DEVEL_APP_ID}.gschema.xml")),
        devel_schema(&schema),
    )
}

/// The release schema under the development ID and path, so the two builds
/// never read or write each other's settings.
fn devel_schema(schema: &str) -> String {
    let id = format!("id=\"{APP_ID}\"");
    let path = format!("path=\"{SETTINGS_PATH}\"");
    assert!(
        schema.contains(&id) && schema.contains(&path),
        "{SCHEMA_FILE} must declare {id} and {path}"
    );
    schema
        .replace(&id, &format!("id=\"{DEVEL_APP_ID}\""))
        .replace(&path, &format!("path=\"{DEVEL_SETTINGS_PATH}\""))
}
