project('gity', 'rust',
  # Single source of truth for the app version: Cargo.toml.
  #
  # We parse Cargo.toml at configure time so Meson packaging/install logic can
  # use the same version without duplicating it here.
  version: run_command(
    'python3',
    [
      '-c',
      '''
import pathlib
import sys

lines = pathlib.Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()

# Extract version from the [package] section only (avoid matching dependency versions).
in_package = False
for raw in lines:
    s = raw.strip()
    if not s or s.startswith('#'):
        continue
    if s.startswith('[') and s.endswith(']'):
        in_package = (s == '[package]')
        continue
    if in_package:
        key, sep, val = s.partition('=')
        if sep and key.strip() == 'version':
            v = val.strip().strip('"').strip("'")
            if not v:
                raise SystemExit('Cargo.toml package version is empty')
            print(v)
            raise SystemExit(0)

raise SystemExit('Cargo.toml missing package version in [package] section')
''',
      meson.current_source_dir() / 'Cargo.toml',
    ],
    check: true,
  ).stdout().strip(),
  license: 'GPL-3.0-or-later',
  meson_version: '>= 1.0.0',
)

# Import modules
gnome = import('gnome')

# Dependencies (for pkg-config checks, actual Rust deps come from Cargo.toml)
gtk4_dep = dependency('gtk4', version: '>= 4.12')
adw_dep = dependency('libadwaita-1', version: '>= 1.5')
glib_dep = dependency('glib-2.0')
gio_dep = dependency('gio-2.0')

# Find cargo
cargo = find_program('cargo', required: true)

# Determine build profile based on buildtype
build_profile = get_option('buildtype') == 'debug' ? 'debug' : 'release'
cargo_build_dir = meson.current_build_dir() / 'target' / build_profile
gity_binary = cargo_build_dir / 'gity'

# GSettings schema directory setup
schema_dir = meson.current_build_dir() / 'schemas'

# Compile schemas in build directory for development
# gnome.compile_schemas compiles to builddir by default
schema_compile = gnome.compile_schemas(
  build_by_default: true,
  depend_files: files('data/com.markdeepwell.gity.gschema.xml'),
)

# The compiled schema will be in the build directory as gschemas.compiled
# For consistency, we'll reference the build directory for schemas

# Schema installation
install_data(
  'data/com.markdeepwell.gity.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

# Desktop file installation
install_data(
  'data/com.markdeepwell.gity.desktop',
  install_dir: get_option('datadir') / 'applications',
)

# Icon installation (SVG for scalable icons)
install_data(
  'data/icons/app.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
  rename: 'com.markdeepwell.gity.svg',
)

# Compile schemas in install location
install_schema_dir = get_option('prefix') / get_option('datadir') / 'glib-2.0' / 'schemas'
meson.add_install_script(
  'glib-compile-schemas',
  '@0@'.format(install_schema_dir),
)

# Set up environment for GSettings schema directory
# For development builds, use the build directory (where gschemas.compiled is)
# For installed builds, use the system directory
if get_option('buildtype') == 'debug'
  gsettings_schema_dir_env = 'GSETTINGS_SCHEMA_DIR=@0@'.format(meson.current_build_dir())
else
  installed_schema_dir = get_option('prefix') / get_option('datadir') / 'glib-2.0' / 'schemas'
  gsettings_schema_dir_env = 'GSETTINGS_SCHEMA_DIR=@0@'.format(installed_schema_dir)
endif

# Build Rust executable using Cargo
# The build.rs script handles resource and schema compilation
cargo_build_args = [
  'build',
  '--manifest-path', meson.current_source_dir() / 'Cargo.toml',
  '--target-dir', meson.current_build_dir() / 'target',
]

if build_profile == 'release'
  cargo_build_args += ['--release']
endif

cargo_build = custom_target(
  'cargo-build',
  build_by_default: true,
  build_always_stale: true,
  output: 'gity',
  command: [
    cargo,
    cargo_build_args,
  ],
  env: {
    'GSETTINGS_SCHEMA_DIR': '@0@'.format(meson.current_build_dir()),
  },
  console: true,
)

# Install script that copies the binary
install_script_path = meson.current_source_dir() / 'meson-install.sh'
bindir_str = get_option('bindir')
meson.add_install_script(
  install_script_path,
  build_profile,
  bindir_str,
)

# For development, create a runnable target
run_script_path = meson.current_source_dir() / 'meson-run.sh'
run_target(
  'run',
  command: [
    run_script_path,
    build_profile,
  ],
  depends: [cargo_build],
)

