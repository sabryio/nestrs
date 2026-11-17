//! The tutorial's quoted code is the example's code.
//!
//! A fenced block directly under `<!-- source: path -->` must appear verbatim, as consecutive
//! lines, in that file. When the example changes, the tutorial fails here instead of silently
//! teaching something that no longer compiles. Skipped when the docs are not next to this crate.

use std::fs;
use std::path::{Path, PathBuf};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// `(markdown file, source path, block text)` for every quoted block.
fn quoted_blocks(dir: &Path) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    let mut files: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    files.sort();
    for file in files {
        let text = fs::read_to_string(&file).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let mut index = 0;
        while index < lines.len() {
            if let Some(source) = lines[index]
                .strip_prefix("<!-- source: ")
                .and_then(|rest| rest.strip_suffix(" -->"))
            {
                assert!(
                    lines.get(index + 1).is_some_and(|line| line.starts_with("```")),
                    "{}: a source marker must be followed by a code fence",
                    file.display()
                );
                let body: Vec<&str> = lines[index + 2..]
                    .iter()
                    .take_while(|line| !line.starts_with("```"))
                    .copied()
                    .collect();
                index += body.len() + 2;
                out.push((
                    file.file_name().unwrap().to_string_lossy().into_owned(),
                    source.to_owned(),
                    body.join("\n"),
                ));
            }
            index += 1;
        }
    }
    out
}

#[test]
fn every_quoted_block_matches_its_source() {
    let dir = repo().join("docs/tutorial");
    if !dir.exists() {
        return;
    }
    let blocks = quoted_blocks(&dir);
    assert!(
        blocks.len() >= 20,
        "the tutorial quotes the example; found {}",
        blocks.len()
    );
    for (chapter, source, body) in blocks {
        let path = repo().join(&source);
        let actual = fs::read_to_string(&path).unwrap_or_else(|_| panic!("{chapter}: {source} does not exist"));
        assert!(
            actual.contains(&body),
            "{chapter} quotes {source}, but the file no longer contains this block verbatim:\n{body}"
        );
    }
}

#[test]
fn every_chapter_is_linked_from_the_index() {
    let dir = repo().join("docs/tutorial");
    if !dir.exists() {
        return;
    }
    let index = fs::read_to_string(dir.join("README.md")).unwrap();
    for entry in fs::read_dir(&dir).unwrap().filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".md") && name != "README.md" {
            assert!(
                index.contains(&format!("({name})")),
                "{name} is not linked from the index"
            );
        }
    }
}
