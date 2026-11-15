//! Where files live: a port with a disk adapter and an in-memory one.

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

/// Port: the files a generator reads and writes, addressed relative to a root.
///
/// Contract: `read` returns `Ok(None)` for a missing file; `write` creates missing
/// parent directories and replaces any existing content.
pub trait Workspace {
    /// The file's text, or `None` if it does not exist.
    fn read(&self, path: &Path) -> io::Result<Option<String>>;
    /// Writes `contents`, creating directories as needed.
    fn write(&mut self, path: &Path, contents: &str) -> io::Result<()>;
    /// Formats files that were just created. Best effort: a workspace that cannot (memory) does
    /// nothing, and so does one whose formatter is missing.
    fn format(&mut self, _paths: &[PathBuf]) {}
}

/// The real file system under `root`.
#[derive(Debug, Clone)]
pub struct DiskWorkspace {
    root: PathBuf,
}

impl DiskWorkspace {
    /// Files are resolved against `root`.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl Workspace for DiskWorkspace {
    fn read(&self, path: &Path) -> io::Result<Option<String>> {
        match std::fs::read_to_string(self.root.join(path)) {
            Ok(text) => Ok(Some(text)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn write(&mut self, path: &Path, contents: &str) -> io::Result<()> {
        let full = self.root.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(full, contents)
    }

    fn format(&mut self, paths: &[PathBuf]) {
        let files: Vec<_> = paths
            .iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "rs"))
            .map(|path| self.root.join(path))
            .collect();
        if files.is_empty() {
            return;
        }
        // Edition 2024 is what generated projects use; a missing rustfmt is not an error.
        let _ = std::process::Command::new("rustfmt")
            .args(["--edition", "2024"])
            .args(&files)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

/// A workspace in memory, for tests and `--dry-run`.
#[derive(Debug, Default, Clone)]
pub struct MemoryWorkspace {
    files: BTreeMap<PathBuf, String>,
}

impl MemoryWorkspace {
    /// An empty workspace.
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts with one file.
    pub fn with(mut self, path: impl Into<PathBuf>, contents: &str) -> Self {
        self.files.insert(path.into(), contents.to_owned());
        self
    }

    /// The file's text, if present.
    pub fn get(&self, path: &str) -> Option<&str> {
        self.files.get(Path::new(path)).map(String::as_str)
    }

    /// Every path, sorted.
    pub fn paths(&self) -> Vec<String> {
        self.files.keys().map(|path| path.display().to_string()).collect()
    }
}

impl Workspace for MemoryWorkspace {
    fn read(&self, path: &Path) -> io::Result<Option<String>> {
        Ok(self.files.get(path).cloned())
    }

    fn write(&mut self, path: &Path, contents: &str) -> io::Result<()> {
        self.files.insert(path.to_owned(), contents.to_owned());
        Ok(())
    }
}
