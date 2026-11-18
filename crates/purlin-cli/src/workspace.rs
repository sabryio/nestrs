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
    /// Every `.rs` file under `dir`, recursively, sorted. Empty when `dir` does not exist.
    fn rust_files(&self, dir: &Path) -> io::Result<Vec<PathBuf>>;
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

    fn rust_files(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        fn walk(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
            let entries = match std::fs::read_dir(root.join(relative)) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
                Err(error) => return Err(error),
            };
            for entry in entries {
                let entry = entry?;
                let path = relative.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    walk(root, &path, out)?;
                } else if path.extension().is_some_and(|ext| ext == "rs") {
                    out.push(path);
                }
            }
            Ok(())
        }
        let mut out = Vec::new();
        walk(&self.root, dir, &mut out)?;
        out.sort();
        Ok(out)
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

    fn rust_files(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        Ok(self
            .files
            .keys()
            .filter(|path| path.starts_with(dir) && path.extension().is_some_and(|ext| ext == "rs"))
            .cloned()
            .collect())
    }

    fn write(&mut self, path: &Path, contents: &str) -> io::Result<()> {
        self.files.insert(path.to_owned(), contents.to_owned());
        Ok(())
    }
}

/// A workspace layered over another: reads fall through, writes stay here. It is how `--dry-run`
/// lets a plan see its own earlier changes without touching disk.
pub struct Overlay<'a> {
    base: &'a dyn Workspace,
    changes: BTreeMap<PathBuf, String>,
}

impl<'a> Overlay<'a> {
    /// An overlay with no changes yet.
    pub fn new(base: &'a dyn Workspace) -> Self {
        Self {
            base,
            changes: BTreeMap::new(),
        }
    }

    /// Unified diff of every change against the base, in path order.
    pub fn diff(&self) -> io::Result<String> {
        let mut out = String::new();
        for (path, after) in &self.changes {
            let before = self.base.read(path)?;
            out.push_str(&crate::diff::unified(
                &path.display().to_string(),
                before.as_deref(),
                after,
            ));
        }
        Ok(out)
    }
}

impl Workspace for Overlay<'_> {
    fn read(&self, path: &Path) -> io::Result<Option<String>> {
        match self.changes.get(path) {
            Some(text) => Ok(Some(text.clone())),
            None => self.base.read(path),
        }
    }

    fn write(&mut self, path: &Path, contents: &str) -> io::Result<()> {
        self.changes.insert(path.to_owned(), contents.to_owned());
        Ok(())
    }

    fn rust_files(&self, dir: &Path) -> io::Result<Vec<PathBuf>> {
        let mut files = self.base.rust_files(dir)?;
        for path in self.changes.keys() {
            if path.starts_with(dir) && path.extension().is_some_and(|ext| ext == "rs") && !files.contains(path) {
                files.push(path.clone());
            }
        }
        files.sort();
        Ok(files)
    }
}
