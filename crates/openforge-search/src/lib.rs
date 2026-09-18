use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub path: String,
    pub score: f64,
    pub matched_terms: Vec<String>,
    pub line: usize,
    pub snippet: String,
}

#[derive(Debug, Clone)]
struct Document {
    path: String,
    content: String,
    term_frequency: HashMap<String, usize>,
    length: usize,
}

#[derive(Debug, Clone)]
pub struct SearchIndex {
    documents: Vec<Document>,
    document_frequency: HashMap<String, usize>,
    average_document_length: f64,
}

impl SearchIndex {
    pub fn build(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .context("search root unavailable")?;
        let mut documents = Vec::new();
        let mut document_frequency: HashMap<String, usize> = HashMap::new();

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
            if path.components().any(|component| component.as_os_str() == ".git") {
                continue;
            }

            let metadata = fs::metadata(path)?;
            if metadata.len() > 2 * 1024 * 1024 {
                continue;
            }

            let bytes = fs::read(path)?;
            if bytes.iter().take(8192).any(|byte| *byte == 0) {
                continue;
            }
            let content = match String::from_utf8(bytes) {
                Ok(value) => value,
                Err(_) => continue,
            };
            let tokens = tokenize(&content);
            if tokens.is_empty() {
                continue;
            }

            let mut term_frequency = HashMap::new();
            let mut seen = HashSet::new();
            for token in &tokens {
                *term_frequency.entry(token.clone()).or_insert(0usize) += 1;
                seen.insert(token.clone());
            }
            for token in seen {
                *document_frequency.entry(token).or_insert(0) += 1;
            }

            documents.push(Document {
                path: path
                    .strip_prefix(&root)?
                    .to_string_lossy()
                    .replace('\\', "/"),
                content,
                term_frequency,
                length: tokens.len(),
            });
        }

        documents.sort_by(|left, right| left.path.cmp(&right.path));
        let average_document_length = if documents.is_empty() {
            0.0
        } else {
            documents.iter().map(|document| document.length).sum::<usize>() as f64
                / documents.len() as f64
        };

        Ok(Self {
            documents,
            document_frequency,
            average_document_length,
        })
    }

    pub fn query(&self, query: &str, limit: usize) -> Vec<SearchHit> {
        let query_terms = tokenize(query);
        if query_terms.is_empty() || self.documents.is_empty() {
            return Vec::new();
        }

        let unique_query: HashSet<String> = query_terms.into_iter().collect();
        let total_documents = self.documents.len() as f64;
        let k1 = 1.5;
        let b = 0.75;
        let mut hits = Vec::new();

        for document in &self.documents {
            let mut score = 0.0;
            let mut matched_terms = Vec::new();

            for term in &unique_query {
                let tf = document.term_frequency.get(term).copied().unwrap_or(0) as f64;
                if tf == 0.0 {
                    continue;
                }
                let df = self.document_frequency.get(term).copied().unwrap_or(0) as f64;
                let idf = ((total_documents - df + 0.5) / (df + 0.5) + 1.0).ln();
                let normalization = if self.average_document_length > 0.0 {
                    1.0 - b
                        + b * document.length as f64 / self.average_document_length
                } else {
                    1.0
                };
                score += idf * (tf * (k1 + 1.0)) / (tf + k1 * normalization);
                matched_terms.push(term.clone());
            }

            if score <= 0.0 {
                continue;
            }

            let (line, snippet) = best_snippet(&document.content, &matched_terms);
            hits.push(SearchHit {
                path: document.path.clone(),
                score,
                matched_terms,
                line,
                snippet,
            });
        }

        hits.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.path.cmp(&right.path))
        });
        hits.truncate(limit.min(1000));
        hits
    }

    pub fn stats(&self) -> BTreeMap<String, usize> {
        BTreeMap::from([
            ("documents".into(), self.documents.len()),
            ("terms".into(), self.document_frequency.len()),
        ])
    }
}

fn best_snippet(content: &str, terms: &[String]) -> (usize, String) {
    let lower_terms: Vec<String> = terms.iter().map(|term| term.to_lowercase()).collect();
    let mut best: Option<(usize, usize, String)> = None;

    for (index, line) in content.lines().enumerate() {
        let lower = line.to_lowercase();
        let matches = lower_terms
            .iter()
            .filter(|term| lower.contains(term.as_str()))
            .count();
        if matches == 0 {
            continue;
        }

        let candidate = (matches, index + 1, line.trim().to_string());
        if best.as_ref().is_none_or(|existing| {
            candidate.0 > existing.0
                || (candidate.0 == existing.0 && candidate.1 < existing.1)
        }) {
            best = Some(candidate);
        }
    }

    best.map(|(_, line, snippet)| (line, truncate(&snippet, 320)))
        .unwrap_or((1, String::new()))
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !character.is_alphanumeric() && character != '_')
        .filter_map(|token| {
            let token = token.trim().to_lowercase();
            (token.len() >= 2).then_some(token)
        })
        .collect()
}

fn truncate(value: &str, max: usize) -> String {
    if value.len() <= max {
        return value.to_string();
    }
    let mut end = max;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &value[..end])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenization_is_case_insensitive_and_symbol_aware() {
        assert_eq!(
            tokenize("OpenForge::Task_Node task-node"),
            vec!["openforge", "task_node", "task", "node"]
        );
    }
}
