//! File access abstraction.
//!
//! The extractor reads sources through a [`FileProvider`] so the language
//! server can overlay unsaved editor buffers on top of the filesystem, and
//! tests can run against in-memory trees.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Read access to source files.
pub trait FileProvider: Sync {
    /// Returns the file contents, or `None` when it does not exist.
    fn read(&self, path: &Path) -> Option<Arc<str>>;

    /// Whether the file exists.
    fn exists(&self, path: &Path) -> bool {
        self.read(path).is_some()
    }
}

/// Reads straight from the filesystem.
#[derive(Clone, Copy, Debug, Default)]
pub struct DiskFileProvider;

impl FileProvider for DiskFileProvider {
    fn read(&self, path: &Path) -> Option<Arc<str>> {
        std::fs::read_to_string(path).ok().map(Arc::from)
    }

    fn exists(&self, path: &Path) -> bool {
        path.is_file()
    }
}

/// In-memory provider for tests, and overlay base for the language server.
#[derive(Clone, Debug, Default)]
pub struct MemoryFileProvider {
    files: HashMap<PathBuf, Arc<str>>,
}

impl MemoryFileProvider {
    pub fn insert(&mut self, path: PathBuf, contents: String) {
        self.files.insert(path, Arc::from(contents));
    }

    pub fn remove(&mut self, path: &Path) {
        self.files.remove(path);
    }
}

impl FileProvider for MemoryFileProvider {
    fn read(&self, path: &Path) -> Option<Arc<str>> {
        self.files.get(path).cloned()
    }
}

/// Editor overlays on top of another provider.
pub struct OverlayFileProvider<'a> {
    pub base: &'a dyn FileProvider,
    pub overlay: &'a MemoryFileProvider,
}

impl FileProvider for OverlayFileProvider<'_> {
    fn read(&self, path: &Path) -> Option<Arc<str>> {
        self.overlay.read(path).or_else(|| self.base.read(path))
    }

    fn exists(&self, path: &Path) -> bool {
        self.overlay.exists(path) || self.base.exists(path)
    }
}
