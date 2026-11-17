//! A small unified diff, for `--dry-run` and `--check`: what would change, line by line.

const CONTEXT: usize = 3;
/// Above this many table cells the diff falls back to "everything changed" rather than
/// spending memory on a file nobody would review line by line.
const MAX_CELLS: usize = 16_000_000;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Edit<'a> {
    Same(&'a str),
    Remove(&'a str),
    Add(&'a str),
}

fn edits<'a>(before: &[&'a str], after: &[&'a str]) -> Vec<Edit<'a>> {
    let (n, m) = (before.len(), after.len());
    if n.saturating_mul(m) > MAX_CELLS {
        return before
            .iter()
            .map(|line| Edit::Remove(line))
            .chain(after.iter().map(|line| Edit::Add(line)))
            .collect();
    }
    // lcs[i][j]: length of the longest common subsequence of before[i..] and after[j..].
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if before[i] == after[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if before[i] == after[j] {
            out.push(Edit::Same(before[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(Edit::Remove(before[i]));
            i += 1;
        } else {
            out.push(Edit::Add(after[j]));
            j += 1;
        }
    }
    out.extend(before[i..].iter().map(|line| Edit::Remove(line)));
    out.extend(after[j..].iter().map(|line| Edit::Add(line)));
    out
}

/// A unified diff of `before` → `after` for `path`, or an empty string when they are equal.
/// `before` is `None` for a file that does not exist yet.
pub fn unified(path: &str, before: Option<&str>, after: &str) -> String {
    if before == Some(after) {
        return String::new();
    }
    let old: Vec<&str> = before.unwrap_or("").lines().collect();
    let new: Vec<&str> = after.lines().collect();
    let edits = edits(&old, &new);

    let mut out = String::new();
    out.push_str(&format!("--- {}\n", if before.is_some() { path } else { "/dev/null" }));
    out.push_str(&format!("+++ {path}\n"));

    // Hunks: every change plus CONTEXT lines around it, merged when they touch.
    let changed: Vec<usize> = edits
        .iter()
        .enumerate()
        .filter(|(_, edit)| !matches!(edit, Edit::Same(_)))
        .map(|(index, _)| index)
        .collect();
    let mut index = 0;
    while index < changed.len() {
        let start = changed[index].saturating_sub(CONTEXT);
        let mut end = (changed[index] + CONTEXT + 1).min(edits.len());
        let mut next = index + 1;
        while next < changed.len() && changed[next].saturating_sub(CONTEXT) <= end {
            end = (changed[next] + CONTEXT + 1).min(edits.len());
            next += 1;
        }
        let (mut old_start, mut new_start) = (1, 1);
        for edit in &edits[..start] {
            match edit {
                Edit::Same(_) => {
                    old_start += 1;
                    new_start += 1;
                }
                Edit::Remove(_) => old_start += 1,
                Edit::Add(_) => new_start += 1,
            }
        }
        let old_len = edits[start..end].iter().filter(|e| !matches!(e, Edit::Add(_))).count();
        let new_len = edits[start..end]
            .iter()
            .filter(|e| !matches!(e, Edit::Remove(_)))
            .count();
        out.push_str(&format!("@@ -{old_start},{old_len} +{new_start},{new_len} @@\n"));
        for edit in &edits[start..end] {
            match edit {
                Edit::Same(line) => out.push_str(&format!(" {line}\n")),
                Edit::Remove(line) => out.push_str(&format!("-{line}\n")),
                Edit::Add(line) => out.push_str(&format!("+{line}\n")),
            }
        }
        index = next;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_text_has_no_diff() {
        assert_eq!(unified("a.rs", Some("x\n"), "x\n"), "");
    }

    #[test]
    fn a_new_file_is_all_additions() {
        let diff = unified("a.rs", None, "one\ntwo\n");
        assert!(
            diff.starts_with("--- /dev/null\n+++ a.rs\n@@ -1,0 +1,2 @@\n+one\n+two\n"),
            "{diff}"
        );
    }

    #[test]
    fn a_change_shows_context_and_keeps_distant_hunks_apart() {
        let before: String = (1..=20).map(|n| format!("line {n}\n")).collect();
        let after = before.replace("line 2\n", "LINE 2\n").replace("line 19\n", "LINE 19\n");
        let diff = unified("f", Some(&before), &after);
        assert_eq!(diff.matches("@@").count(), 4, "two hunks: {diff}");
        assert!(diff.contains("-line 2\n+LINE 2\n"), "{diff}");
        assert!(diff.contains(" line 1\n"), "context before: {diff}");
        assert!(!diff.contains("line 10"), "far lines stay out: {diff}");
    }

    #[test]
    fn insertions_in_the_middle_do_not_mark_the_rest_as_changed() {
        let diff = unified("f", Some("a\nb\nc\n"), "a\nX\nb\nc\n");
        assert!(
            diff.contains("+X\n") && !diff.contains("-b") && !diff.contains("-c"),
            "{diff}"
        );
    }
}
