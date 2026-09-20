use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FileRecord {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub language: String,
    pub lines: usize,
    pub is_test: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryIndex {
    pub root: String,
    pub fingerprint: String,
    pub files: Vec<FileRecord>,
    pub language_counts: BTreeMap<String, usize>,
    pub total_bytes: u64,
    pub total_lines: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RepositoryDelta {
    pub added: Vec<FileRecord>,
    pub modified: Vec<FileChange>,
    pub deleted: Vec<FileRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub before: FileRecord,
    pub after: FileRecord,
}

impl RepositoryDelta {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.modified.is_empty() && self.deleted.is_empty()
    }

    pub fn changed_paths(&self) -> Vec<String> {
        let mut paths: Vec<String> = self
            .added
            .iter()
            .map(|record| record.path.clone())
            .chain(self.modified.iter().map(|change| change.after.path.clone()))
            .chain(self.deleted.iter().map(|record| record.path.clone()))
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }
}

impl RepositoryIndex {
    pub fn build(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .context("repository root unavailable")?;
        let mut files = Vec::new();
        let mut language_counts = BTreeMap::new();
        let mut total_bytes = 0u64;
        let mut total_lines = 0usize;

        for entry in WalkBuilder::new(&root)
            .hidden(false)
            .git_ignore(true)
            .git_exclude(true)
            .build()
        {
            let entry = entry?;
            if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                continue;
            }

            let path = entry.path();
            if path
                .components()
                .any(|component| component.as_os_str() == ".git")
            {
                continue;
            }

            let data = fs::read(path)?;
            if data.len() > 5 * 1024 * 1024 || data.iter().take(8192).any(|byte| *byte == 0) {
                continue;
            }

            let relative = path
                .strip_prefix(&root)?
                .to_string_lossy()
                .replace('\\', "/");
            let language = language_for(path);
            let lines = byte_line_count(&data);
            let is_test = is_test_path(&relative);

            *language_counts.entry(language.clone()).or_insert(0) += 1;
            total_bytes = total_bytes.saturating_add(data.len() as u64);
            total_lines = total_lines.saturating_add(lines);
            files.push(FileRecord {
                path: relative,
                bytes: data.len() as u64,
                sha256: hex::encode(Sha256::digest(&data)),
                language,
                lines,
                is_test,
            });
        }

        files.sort_by(|left, right| left.path.cmp(&right.path));
        let fingerprint = fingerprint(&files)?;

        Ok(Self {
            root: root.display().to_string(),
            fingerprint,
            files,
            language_counts,
            total_bytes,
            total_lines,
        })
    }

    pub fn diff(&self, newer: &RepositoryIndex) -> RepositoryDelta {
        let old: HashMap<&str, &FileRecord> = self
            .files
            .iter()
            .map(|record| (record.path.as_str(), record))
            .collect();
        let new: HashMap<&str, &FileRecord> = newer
            .files
            .iter()
            .map(|record| (record.path.as_str(), record))
            .collect();

        let mut delta = RepositoryDelta::default();

        for record in &newer.files {
            match old.get(record.path.as_str()) {
                None => delta.added.push(record.clone()),
                Some(before) if before.sha256 != record.sha256 => {
                    delta.modified.push(FileChange {
                        before: (*before).clone(),
                        after: record.clone(),
                    });
                }
                _ => {}
            }
        }

        for record in &self.files {
            if !new.contains_key(record.path.as_str()) {
                delta.deleted.push(record.clone());
            }
        }

        delta.added.sort_by(|a, b| a.path.cmp(&b.path));
        delta
            .modified
            .sort_by(|a, b| a.after.path.cmp(&b.after.path));
        delta.deleted.sort_by(|a, b| a.path.cmp(&b.path));
        delta
    }

    pub fn relevant_files(&self, query: &str, limit: usize) -> Vec<String> {
        let terms: BTreeSet<String> = query
            .split(|character: char| !character.is_alphanumeric() && character != '_')
            .filter(|term| term.len() > 2)
            .map(|term| term.to_lowercase())
            .collect();

        let mut scored: Vec<(i32, String)> = self
            .files
            .iter()
            .filter_map(|record| {
                let path = record.path.to_lowercase();
                let language = record.language.to_lowercase();
                let score = terms
                    .iter()
                    .map(|term| {
                        let mut score = 0;
                        if path.contains(term) {
                            score += 6;
                        }
                        if language == *term {
                            score += 3;
                        }
                        score
                    })
                    .sum::<i32>()
                    + if record.is_test { 1 } else { 0 };

                (score > 0).then_some((score, record.path.clone()))
            })
            .collect();

        scored.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| left.1.cmp(&right.1)));
        scored
            .into_iter()
            .take(limit.min(1000))
            .map(|(_, path)| path)
            .collect()
    }

    pub fn file(&self, path: &str) -> Option<&FileRecord> {
        self.files
            .binary_search_by(|record| record.path.as_str().cmp(path))
            .ok()
            .map(|index| &self.files[index])
    }
}

fn fingerprint(files: &[FileRecord]) -> Result<String> {
    let mut hasher = Sha256::new();
    for record in files {
        hasher.update(record.path.as_bytes());
        hasher.update([0]);
        hasher.update(record.sha256.as_bytes());
        hasher.update([0]);
        hasher.update(record.bytes.to_le_bytes());
    }
    Ok(hex::encode(hasher.finalize()))
}

fn byte_line_count(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    bytes.iter().filter(|byte| **byte == b'\n').count() + usize::from(bytes.last() != Some(&b'\n'))
}

fn is_test_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test")
        || lower.starts_with("test")
        || lower.contains("_test.")
        || lower.contains(".test.")
        || lower.contains(".spec.")
}

fn language_for(path: &Path) -> String {
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => "rust",
        "ts" | "tsx" => "typescript",
        "js" | "jsx" => "javascript",
        "py" => "python",
        "go" => "go",
        "java" => "java",
        "kt" | "kts" => "kotlin",
        "rb" => "ruby",
        "php" => "php",
        "cs" => "csharp",
        "c" | "h" => "c",
        "cpp" | "cc" | "hpp" => "cpp",
        "sql" => "sql",
        "html" => "html",
        "css" => "css",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "toml" => "toml",
        "md" => "markdown",
        "sh" => "shell",
        _ => "text",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delta_detects_add_modify_delete() {
        let base = RepositoryIndex {
            root: ".".into(),
            fingerprint: "old".into(),
            files: vec![
                FileRecord {
                    path: "a.rs".into(),
                    bytes: 1,
                    sha256: "a".into(),
                    language: "rust".into(),
                    lines: 1,
                    is_test: false,
                },
                FileRecord {
                    path: "b.rs".into(),
                    bytes: 1,
                    sha256: "b".into(),
                    language: "rust".into(),
                    lines: 1,
                    is_test: false,
                },
            ],
            language_counts: BTreeMap::new(),
            total_bytes: 2,
            total_lines: 2,
        };
        let newer = RepositoryIndex {
            root: ".".into(),
            fingerprint: "new".into(),
            files: vec![
                FileRecord {
                    path: "a.rs".into(),
                    bytes: 2,
                    sha256: "changed".into(),
                    language: "rust".into(),
                    lines: 1,
                    is_test: false,
                },
                FileRecord {
                    path: "c.rs".into(),
                    bytes: 1,
                    sha256: "c".into(),
                    language: "rust".into(),
                    lines: 1,
                    is_test: false,
                },
            ],
            language_counts: BTreeMap::new(),
            total_bytes: 3,
            total_lines: 2,
        };

        let delta = base.diff(&newer);
        assert_eq!(delta.modified.len(), 1);
        assert_eq!(delta.added[0].path, "c.rs");
        assert_eq!(delta.deleted[0].path, "b.rs");
    }
}
