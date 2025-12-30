use std::process::Command;

fn main() {
    glib_build_tools::compile_resources(
        &["src/resources"],
        "src/resources/resources.gresource.xml",
        "resources.gresource",
    );

    // Tell Cargo to rerun this build script if the schema file changes
    println!("cargo:rerun-if-changed=data/com.markdeepwell.GitY.gschema.xml");

    // Compile GSettings schemas
    // For development, compile to both debug and release directories
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());
    let output_dir = std::path::Path::new("target").join(&profile);
    let schemas_dir = output_dir.join("schemas");

    std::fs::create_dir_all(&schemas_dir).unwrap_or(());

    let status = Command::new("glib-compile-schemas")
        .arg("--targetdir")
        .arg(&schemas_dir)
        .arg("data")
        .status();

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
            eprintln!("Warning: Failed to run glib-compile-schemas: {}", e);
            eprintln!(
                "Make sure glib-compile-schemas is installed (usually part of glib2-devel or glib2-tools)"
            );
            eprintln!("GSettings functionality will not work without compiled schemas");
        }
    }
}
