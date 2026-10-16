//! Side-by-side before/after previews for image files in a diff.
//!
//! Blobs are read and decoded on a small worker pool so expanding a section
//! with large images never blocks the UI. Decoding goes through glycin, which
//! runs the image loaders in a sandboxed process: images come from whatever
//! repository was cloned, so a malformed or malicious file must not be able to
//! affect GitY itself. Each pane shows a spinner until its image arrives, then
//! the picture scaled down to fit (never up) above its pixel dimensions and
//! file size.

use git2::Oid;
use gtk::{gdk, glib, prelude::*};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, mpsc};

use crate::git;
use crate::logger::Logger;

/// Extensions previewed as images. Which ones decode depends on the glycin
/// loaders installed on the system or in the Flatpak runtime; any that fail
/// show a message instead.
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tif", "tiff", "svg", "avif", "jxl",
];

/// Blobs larger than this are not loaded for preview.
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Tall images are scaled down so they are at most this many pixels high.
const MAX_HEIGHT_PX: i32 = 480;

/// Images wider or taller than this are not decoded. The byte cap alone
/// doesn't bound memory: a small, highly compressed file can declare huge
/// dimensions and decode to gigabytes of pixels.
const MAX_DIMENSION_PX: u32 = 8192;

const TOO_LARGE: &str = "Image too large to preview";
const UNAVAILABLE: &str = "Preview unavailable";

/// Returns whether `path` has an image extension we try to preview.
pub fn is_image_path(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.iter().any(|e| ext.eq_ignore_ascii_case(e)))
}

/// Blob ids of the two versions of an image; `None` for a side that does not
/// exist (added or deleted file).
#[derive(Debug, Clone, PartialEq)]
pub struct ImageDiffSource {
    pub old: Option<Oid>,
    pub new: Option<Oid>,
}

/// One version of the image, shown as a pane.
struct Side {
    oid: Oid,
    heading: Option<&'static str>,
    css_class: Option<&'static str>,
}

fn sides(source: &ImageDiffSource) -> Vec<Side> {
    let side = |oid, heading, css_class| Side {
        oid,
        heading: Some(heading),
        css_class: Some(css_class),
    };
    match (source.old, source.new) {
        // Renamed or copied without content changes: one unlabeled pane.
        (Some(old), Some(new)) if old == new => vec![Side {
            oid: new,
            heading: None,
            css_class: None,
        }],
        (Some(old), Some(new)) => vec![
            side(old, "Before", "image-diff-before"),
            side(new, "After", "image-diff-after"),
        ],
        (Some(old), None) => vec![side(old, "Deleted", "image-diff-before")],
        (None, Some(new)) => vec![side(new, "Added", "image-diff-after")],
        (None, None) => Vec::new(),
    }
}

struct LoadedImage {
    texture: gdk::Texture,
    bytes: usize,
}

type LoadResult = Result<LoadedImage, &'static str>;

fn load_image(repo_path: &Path, oid: Oid) -> LoadResult {
    let bytes = match git::read_blob(repo_path, oid, MAX_IMAGE_BYTES) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return Err(TOO_LARGE),
        Err(e) => {
            Logger::error(&format!("Failed to read image blob {oid}: {e}"));
            return Err(UNAVAILABLE);
        }
    };
    let size = bytes.len();
    match decode(bytes) {
        Ok(texture) => Ok(LoadedImage {
            texture,
            bytes: size,
        }),
        Err(e) => {
            Logger::info(&format!("Cannot decode image blob {oid}: {e}"));
            Err(UNAVAILABLE)
        }
    }
}

/// Runtime for glycin's async API. Glycin pools its loader processes for the
/// whole process and runs their tasks on the runtime that started them, so
/// this must outlive every window (unlike the per-window `AppState` runtime).
fn glycin_runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("glycin")
            .enable_all()
            .build()
            .expect("create glycin runtime")
    })
}

/// Decodes the first frame of an image in glycin's sandboxed loader process.
fn decode(bytes: Vec<u8>) -> Result<gdk::Texture, glycin::Error> {
    glycin_runtime().block_on(async {
        let mut loader = glycin::Loader::new_vec(bytes);
        loader
            .limits(glycin::Limits::default().max_dimensions((MAX_DIMENSION_PX, MAX_DIMENSION_PX)));
        // By default glycin would run on GTK's main loop, which has no tokio
        // runtime; its managed thread inherits this one instead.
        loader.main_context_selector(glycin::MainContextSelector::Managed);
        let mut image = loader.load().await?;
        Ok(image.next_frame().await?.texture())
    })
}

/// Shared pool for image loading, capped so "Expand all" on a commit with
/// many images doesn't decode them all at once.
fn loader_pool() -> &'static glib::ThreadPool {
    static POOL: OnceLock<glib::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| glib::ThreadPool::shared(Some(2)).expect("create image loader pool"))
}

struct Pane {
    /// Holds the spinner while loading, then the picture or a message.
    slot: gtk::Box,
    caption: gtk::Label,
    alt_text: String,
    css_class: Option<&'static str>,
}

fn build_pane(side: &Side, label: &str) -> (gtk::Box, Pane) {
    let pane = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Start)
        .spacing(6)
        .build();

    if let Some(heading) = side.heading {
        let heading_label = gtk::Label::new(Some(heading));
        heading_label.add_css_class("caption-heading");
        heading_label.add_css_class("dim-label");
        pane.append(&heading_label);
    }

    let slot = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    // adw::Spinner expands to fill its allocation, so constrain it.
    let spinner = adw::Spinner::builder()
        .width_request(32)
        .height_request(32)
        .halign(gtk::Align::Center)
        .build();
    slot.append(&spinner);
    pane.append(&slot);

    let caption = gtk::Label::builder().visible(false).build();
    caption.add_css_class("caption");
    caption.add_css_class("dim-label");
    pane.append(&caption);

    let alt_text = match side.heading {
        Some(heading) => format!("{heading}: {label}"),
        None => label.to_string(),
    };
    (
        pane,
        Pane {
            slot,
            caption,
            alt_text,
            css_class: side.css_class,
        },
    )
}

impl Pane {
    fn show(&self, result: LoadResult) {
        while let Some(child) = self.slot.first_child() {
            self.slot.remove(&child);
        }

        let image = match result {
            Ok(image) => image,
            Err(message) => {
                let label = gtk::Label::new(Some(message));
                label.add_css_class("dim-label");
                self.slot.append(&label);
                return;
            }
        };

        let (width, height) = (image.texture.width(), image.texture.height());
        let picture = gtk::Picture::builder()
            .paintable(&image.texture)
            .content_fit(gtk::ContentFit::ScaleDown)
            .can_shrink(true)
            .alternative_text(self.alt_text.as_str())
            .build();
        picture.add_css_class("image-diff-canvas");
        if let Some(css_class) = self.css_class {
            picture.add_css_class(css_class);
        }

        // A Picture asks for the full column width at its aspect ratio, which
        // would stretch small images and let tall ones take over the diff.
        // Clamp it to its own width, or less if that would exceed the height cap.
        let max_width = width.min(MAX_HEIGHT_PX * width / height.max(1)).max(1);
        let clamp = adw::Clamp::builder()
            .orientation(gtk::Orientation::Horizontal)
            .unit(adw::LengthUnit::Px)
            .maximum_size(max_width)
            .tightening_threshold(max_width)
            .child(&picture)
            .build();
        self.slot.append(&clamp);

        self.caption.set_text(&format!(
            "{width} × {height} · {}",
            glib::format_size(image.bytes as u64)
        ));
        self.caption.set_visible(true);
    }
}

/// Builds the before/after comparison for one file and starts loading the
/// images. `label` names the file for screen readers. With
/// `has_text_fallback`, the comparison hides itself if no image can be
/// decoded (e.g. a Git LFS pointer), leaving only the text diff below it.
pub fn build(
    source: &ImageDiffSource,
    repo_path: &Path,
    label: &str,
    has_text_fallback: bool,
) -> gtk::Box {
    let container = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .homogeneous(true)
        .spacing(12)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build();

    let sides = sides(source);
    let mut panes = Vec::with_capacity(sides.len());
    for side in &sides {
        let (widget, pane) = build_pane(side, label);
        container.append(&widget);
        panes.push(pane);
    }

    let oids: Vec<Oid> = sides.iter().map(|side| side.oid).collect();
    let repo_path: PathBuf = repo_path.to_path_buf();
    let cancelled = Arc::new(AtomicBool::new(false));
    let cancelled_for_job = cancelled.clone();
    let (tx, rx) = mpsc::channel();
    let pushed = loader_pool().push(move || {
        // Skip work queued for a section that was collapsed or replaced.
        if cancelled_for_job.load(Ordering::Relaxed) {
            return;
        }
        let results: Vec<LoadResult> = oids
            .into_iter()
            .map(|oid| load_image(&repo_path, oid))
            .collect();
        let _ = tx.send(results);
    });
    if let Err(e) = pushed {
        Logger::error(&format!("Failed to queue image preview: {e}"));
    }

    poll_images(
        rx,
        container.downgrade(),
        panes,
        cancelled,
        has_text_fallback,
    );
    container
}

fn poll_images(
    rx: mpsc::Receiver<Vec<LoadResult>>,
    container: glib::WeakRef<gtk::Box>,
    panes: Vec<Pane>,
    cancelled: Arc<AtomicBool>,
    has_text_fallback: bool,
) {
    // The section was collapsed or the diff replaced: stop, and let any job
    // still queued know it can skip.
    let Some(container_strong) = container.upgrade() else {
        cancelled.store(true, Ordering::Relaxed);
        return;
    };

    match rx.try_recv() {
        Ok(results) => {
            if has_text_fallback && results.iter().all(Result::is_err) {
                container_strong.set_visible(false);
                return;
            }
            for (pane, result) in panes.iter().zip(results) {
                pane.show(result);
            }
        }
        Err(mpsc::TryRecvError::Empty) => {
            glib::timeout_add_local_once(std::time::Duration::from_millis(50), move || {
                poll_images(rx, container, panes, cancelled, has_text_fallback);
            });
        }
        Err(mpsc::TryRecvError::Disconnected) => {
            if has_text_fallback {
                container_strong.set_visible(false);
                return;
            }
            for pane in &panes {
                pane.show(Err(UNAVAILABLE));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_image_path_matches_known_extensions_case_insensitively() {
        assert!(is_image_path("logo.png"));
        assert!(is_image_path("assets/Photo.JPeG"));
        assert!(is_image_path("icons/app.svg"));
        assert!(is_image_path("photos/shot.JXL"));
    }

    #[test]
    fn is_image_path_rejects_other_files() {
        assert!(!is_image_path("notes.png.txt"));
        assert!(!is_image_path("Makefile"));
        assert!(!is_image_path(".png"));
        assert!(!is_image_path("archive.zip"));
    }

    fn oid(n: u8) -> Oid {
        Oid::from_bytes(&[n; 20]).unwrap()
    }

    fn headings(source: ImageDiffSource) -> Vec<Option<&'static str>> {
        sides(&source).iter().map(|side| side.heading).collect()
    }

    #[test]
    fn sides_label_each_version() {
        let both = ImageDiffSource {
            old: Some(oid(1)),
            new: Some(oid(2)),
        };
        assert_eq!(headings(both), vec![Some("Before"), Some("After")]);

        let added = ImageDiffSource {
            old: None,
            new: Some(oid(2)),
        };
        assert_eq!(headings(added), vec![Some("Added")]);

        let deleted = ImageDiffSource {
            old: Some(oid(1)),
            new: None,
        };
        assert_eq!(headings(deleted), vec![Some("Deleted")]);
    }

    #[test]
    fn sides_show_unchanged_image_once() {
        let renamed = ImageDiffSource {
            old: Some(oid(1)),
            new: Some(oid(1)),
        };
        assert_eq!(headings(renamed), vec![None]);
    }
}
