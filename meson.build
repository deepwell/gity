project('gity', 'rust',
  version: '0.2.1',
  license: 'GPL-3.0-or-later',
  meson_version: '>= 1.0.0',
)

# Import modules
gnome = import('gnome')

# Dependencies (for pkg-config checks, actual Rust deps come from Cargo.toml)
dependency('gtk4', version: '>= 4.12')
dependency('libadwaita-1', version: '>= 1.5')
dependency('glib-2.0')
dependency('gio-2.0')

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
  depend_files: files('data/com.markdeepwell.GitY.gschema.xml'),
)

# The compiled schema will be in the build directory as gschemas.compiled
# For consistency, we'll reference the build directory for schemas

# Schema installation
install_data(
  'data/com.markdeepwell.GitY.gschema.xml',
  install_dir: get_option('datadir') / 'glib-2.0' / 'schemas',
)

# Desktop file installation
install_data(
  'data/com.markdeepwell.GitY.desktop',
  install_dir: get_option('datadir') / 'applications',
)

# Icon installation (SVG for scalable icons)
install_data(
  'data/icons/app.svg',
  install_dir: get_option('datadir') / 'icons' / 'hicolor' / 'scalable' / 'apps',
  rename: 'com.markdeepwell.GitY.svg',
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

