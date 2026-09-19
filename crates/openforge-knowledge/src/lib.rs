use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
    process::Command,
};
use tree_sitter::{Language, Node, Parser};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeNodeKind {
    File,
    Function,
    Method,
    Class,
    Struct,
    Enum,
    Trait,
    Interface,
    TypeAlias,
    Module,
    Constant,
    Variable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeNode {
    pub id: String,
    pub name: String,
    pub kind: KnowledgeNodeKind,
    pub path: String,
    pub language: String,
    pub start_line: usize,
    pub end_line: usize,
    pub signature: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum KnowledgeEdgeKind {
    Contains,
    Imports,
    Calls,
    References,
    ChangedBy,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct KnowledgeEdge {
    pub from: String,
    pub to: String,
    pub kind: KnowledgeEdgeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GitFileHistory {
    pub commits: usize,
    pub authors: BTreeMap<String, usize>,
    pub latest_commit: Option<String>,
    pub latest_unix_timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct KnowledgeGraph {
    pub root: String,
    pub nodes: Vec<KnowledgeNode>,
    pub edges: Vec<KnowledgeEdge>,
    pub git_history: BTreeMap<String, GitFileHistory>,
    pub files_indexed: usize,
}

#[derive(Debug, Clone)]
struct PendingReference {
    source: String,
    target_name: String,
    kind: KnowledgeEdgeKind,
}

impl KnowledgeGraph {
    pub fn build(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .context("knowledge root unavailable")?;

        let mut nodes = Vec::new();
        let mut edges = BTreeSet::new();
        let mut pending = Vec::new();
        let mut files_indexed = 0usize;

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

            let Some(language) = language_for(path) else {
                continue;
            };
            let metadata = fs::metadata(path)?;
            if metadata.len() > 3 * 1024 * 1024 {
                continue;
            }
            let source = match fs::read(path) {
                Ok(bytes) if !bytes.iter().take(8192).any(|byte| *byte == 0) => bytes,
                _ => continue,
            };

            let relative = path
                .strip_prefix(&root)?
                .to_string_lossy()
                .replace('\\', "/");
            let file_id = format!("file:{relative}");
            let file_node = KnowledgeNode {
                id: file_id.clone(),
                name: relative.clone(),
                kind: KnowledgeNodeKind::File,
                path: relative.clone(),
                language: language.to_string(),
                start_line: 1,
                end_line: source.iter().filter(|byte| **byte == b'\n').count() + 1,
                signature: relative.clone(),
                sha256: hex::encode(Sha256::digest(&source)),
            };
            nodes.push(file_node);
            files_indexed += 1;

            let mut parser = Parser::new();
            let Some(grammar) = grammar_for(path) else {
                continue;
            };
            parser
                .set_language(&grammar)
                .with_context(|| format!("load Tree-sitter grammar for {relative}"))?;
            let Some(tree) = parser.parse(&source, None) else {
                continue;
            };

            walk_tree(
                tree.root_node(),
                &source,
                &relative,
                language,
                &file_id,
                None,
                &mut nodes,
                &mut edges,
                &mut pending,
            );
        }

        let by_name = nodes
            .iter()
            .fold(HashMap::<String, Vec<String>>::new(), |mut map, node| {
                map.entry(node.name.clone())
                    .or_default()
                    .push(node.id.clone());
                map
            });

        for reference in pending {
            if let Some(targets) = by_name.get(&reference.target_name) {
                for target in targets {
                    if target != &reference.source {
                        edges.insert(KnowledgeEdge {
                            from: reference.source.clone(),
                            to: target.clone(),
                            kind: reference.kind.clone(),
                        });
                    }
                }
            } else if reference.kind == KnowledgeEdgeKind::Imports {
                edges.insert(KnowledgeEdge {
                    from: reference.source,
                    to: format!("module:{}", reference.target_name),
                    kind: KnowledgeEdgeKind::Imports,
                });
            }
        }

        let git_history = load_git_history(&root).unwrap_or_default();
        for (path, history) in &git_history {
            let file_id = format!("file:{path}");
            for author in history.authors.keys() {
                edges.insert(KnowledgeEdge {
                    from: file_id.clone(),
                    to: format!("author:{author}"),
                    kind: KnowledgeEdgeKind::ChangedBy,
                });
            }
        }

        nodes.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.start_line.cmp(&right.start_line))
                .then_with(|| left.id.cmp(&right.id))
        });

        Ok(Self {
            root: root.display().to_string(),
            nodes,
            edges: edges.into_iter().collect(),
            git_history,
            files_indexed,
        })
    }

    pub fn find_symbols(&self, query: &str, limit: usize) -> Vec<KnowledgeNode> {
        let terms = tokenize(query);
        let mut ranked = self
            .nodes
            .iter()
            .filter(|node| node.kind != KnowledgeNodeKind::File)
            .filter_map(|node| {
                let name = node.name.to_lowercase();
                let path = node.path.to_lowercase();
                let signature = node.signature.to_lowercase();
                let mut score = 0i32;
                for term in &terms {
                    if name == *term {
                        score += 30;
                    } else if name.starts_with(term) {
                        score += 18;
                    } else if name.contains(term) {
                        score += 12;
                    }
                    if path.contains(term) {
                        score += 4;
                    }
                    if signature.contains(term) {
                        score += 2;
                    }
                }
                (score > 0).then_some((score, node.clone()))
            })
            .collect::<Vec<_>>();

        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.path.cmp(&right.1.path))
                .then_with(|| left.1.start_line.cmp(&right.1.start_line))
        });
        ranked
            .into_iter()
            .take(limit.min(1000))
            .map(|(_, node)| node)
            .collect()
    }

    pub fn neighbors(&self, node_id: &str, depth: usize) -> Vec<KnowledgeNode> {
        let depth = depth.min(8);
        let by_id = self
            .nodes
            .iter()
            .map(|node| (node.id.as_str(), node))
            .collect::<HashMap<_, _>>();
        let mut frontier = BTreeSet::from([node_id.to_string()]);
        let mut seen = BTreeSet::from([node_id.to_string()]);

        for _ in 0..depth {
            let mut next = BTreeSet::new();
            for edge in &self.edges {
                if frontier.contains(&edge.from) && !seen.contains(&edge.to) {
                    next.insert(edge.to.clone());
                }
                if frontier.contains(&edge.to) && !seen.contains(&edge.from) {
                    next.insert(edge.from.clone());
                }
            }
            if next.is_empty() {
                break;
            }
            seen.extend(next.iter().cloned());
            frontier = next;
        }

        seen.remove(node_id);
        seen.into_iter()
            .filter_map(|id| by_id.get(id.as_str()).map(|node| (*node).clone()))
            .collect()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbeddingEntry {
    pub id: String,
    pub vector: Vec<f32>,
    #[serde(default)]
    pub metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DenseVectorIndex {
    entries: Vec<EmbeddingEntry>,
}

impl DenseVectorIndex {
    pub fn insert(&mut self, entry: EmbeddingEntry) -> Result<()> {
        if entry.vector.is_empty() || entry.vector.iter().any(|value| !value.is_finite()) {
            anyhow::bail!("embedding vector must be finite and non-empty");
        }
        if let Some(existing) = self.entries.iter_mut().find(|value| value.id == entry.id) {
            *existing = entry;
        } else {
            self.entries.push(entry);
        }
        Ok(())
    }

    pub fn query(&self, vector: &[f32], limit: usize) -> Result<Vec<(String, f32)>> {
        if vector.is_empty() || vector.iter().any(|value| !value.is_finite()) {
            anyhow::bail!("query vector must be finite and non-empty");
        }

        let mut scored = self
            .entries
            .iter()
            .filter(|entry| entry.vector.len() == vector.len())
            .filter_map(|entry| {
                cosine_similarity(vector, &entry.vector).map(|score| (entry.id.clone(), score))
            })
            .collect::<Vec<_>>();

        scored.sort_by(|left, right| {
            right
                .1
                .total_cmp(&left.1)
                .then_with(|| left.0.cmp(&right.0))
        });
        scored.truncate(limit.min(1000));
        Ok(scored)
    }
}

pub fn reciprocal_rank_fusion(rankings: &[Vec<String>], limit: usize) -> Vec<(String, f64)> {
    let mut scores = HashMap::<String, f64>::new();
    for ranking in rankings {
        for (index, id) in ranking.iter().enumerate() {
            *scores.entry(id.clone()).or_insert(0.0) += 1.0 / (60.0 + index as f64 + 1.0);
        }
    }
    let mut values = scores.into_iter().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .1
            .total_cmp(&left.1)
            .then_with(|| left.0.cmp(&right.0))
    });
    values.truncate(limit.min(1000));
    values
}

fn walk_tree(
    node: Node<'_>,
    source: &[u8],
    path: &str,
    language: &str,
    file_id: &str,
    parent_symbol: Option<&str>,
    nodes: &mut Vec<KnowledgeNode>,
    edges: &mut BTreeSet<KnowledgeEdge>,
    pending: &mut Vec<PendingReference>,
) {
    let kind = classify_node(language, node.kind());
    let mut current_symbol = parent_symbol.map(str::to_string);

    if let Some(symbol_kind) = kind {
        let name = node
            .child_by_field_name("name")
            .and_then(|value| value.utf8_text(source).ok())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| node.kind())
            .to_string();

        let start_line = node.start_position().row + 1;
        let end_line = node.end_position().row + 1;
        let signature = compact_text(node.utf8_text(source).unwrap_or_default(), 320);
        let id = symbol_id(path, start_line, node.kind(), &name);
        nodes.push(KnowledgeNode {
            id: id.clone(),
            name,
            kind: symbol_kind,
            path: path.to_string(),
            language: language.to_string(),
            start_line,
            end_line,
            sha256: hex::encode(Sha256::digest(signature.as_bytes())),
            signature,
        });
        edges.insert(KnowledgeEdge {
            from: parent_symbol.unwrap_or(file_id).to_string(),
            to: id.clone(),
            kind: KnowledgeEdgeKind::Contains,
        });
        current_symbol = Some(id);
    }

    if is_import_node(node.kind()) {
        if let Ok(text) = node.utf8_text(source) {
            for target in import_targets(text) {
                pending.push(PendingReference {
                    source: current_symbol.as_deref().unwrap_or(file_id).to_string(),
                    target_name: target,
                    kind: KnowledgeEdgeKind::Imports,
                });
            }
        }
    }

    if is_call_node(node.kind()) {
        if let Some(function) = node
            .child_by_field_name("function")
            .or_else(|| node.child_by_field_name("name"))
        {
            if let Ok(text) = function.utf8_text(source) {
                let target = text
                    .split(|character: char| !(character.is_alphanumeric() || character == '_'))
                    .filter(|value| !value.is_empty())
                    .next_back()
                    .unwrap_or("")
                    .to_string();
                if !target.is_empty() {
                    pending.push(PendingReference {
                        source: current_symbol.as_deref().unwrap_or(file_id).to_string(),
                        target_name: target,
                        kind: KnowledgeEdgeKind::Calls,
                    });
                }
            }
        }
    }

    let mut cursor = node.walk();
    if cursor.goto_first_child() {
        loop {
            let child = cursor.node();
            walk_tree(
                child,
                source,
                path,
                language,
                file_id,
                current_symbol.as_deref(),
                nodes,
                edges,
                pending,
            );
            if !cursor.goto_next_sibling() {
                break;
            }
        }
    }
}

fn grammar_for(path: &Path) -> Option<Language> {
    match path
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => Some(tree_sitter_rust::LANGUAGE.into()),
        "py" => Some(tree_sitter_python::LANGUAGE.into()),
        "js" | "jsx" => Some(tree_sitter_javascript::LANGUAGE.into()),
        "ts" => Some(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
        "tsx" => Some(tree_sitter_typescript::LANGUAGE_TSX.into()),
        _ => None,
    }
}

fn language_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|value| value.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" => Some("rust"),
        "py" => Some("python"),
        "js" | "jsx" => Some("javascript"),
        "ts" | "tsx" => Some("typescript"),
        _ => None,
    }
}

fn classify_node(language: &str, kind: &str) -> Option<KnowledgeNodeKind> {
    match (language, kind) {
        ("rust", "function_item") => Some(KnowledgeNodeKind::Function),
        ("rust", "struct_item") => Some(KnowledgeNodeKind::Struct),
        ("rust", "enum_item") => Some(KnowledgeNodeKind::Enum),
        ("rust", "trait_item") => Some(KnowledgeNodeKind::Trait),
        ("rust", "type_item") => Some(KnowledgeNodeKind::TypeAlias),
        ("rust", "const_item") | ("rust", "static_item") => Some(KnowledgeNodeKind::Constant),
        ("python", "function_definition") => Some(KnowledgeNodeKind::Function),
        ("python", "class_definition") => Some(KnowledgeNodeKind::Class),
        ("javascript", "function_declaration") | ("typescript", "function_declaration") => {
            Some(KnowledgeNodeKind::Function)
        }
        ("javascript", "method_definition") | ("typescript", "method_definition") => {
            Some(KnowledgeNodeKind::Method)
        }
        ("javascript", "class_declaration") | ("typescript", "class_declaration") => {
            Some(KnowledgeNodeKind::Class)
        }
        ("typescript", "interface_declaration") => Some(KnowledgeNodeKind::Interface),
        ("typescript", "type_alias_declaration") => Some(KnowledgeNodeKind::TypeAlias),
        ("typescript", "enum_declaration") => Some(KnowledgeNodeKind::Enum),
        ("javascript", "lexical_declaration") | ("typescript", "lexical_declaration") => {
            Some(KnowledgeNodeKind::Variable)
        }
        _ => None,
    }
}

fn is_import_node(kind: &str) -> bool {
    matches!(
        kind,
        "use_declaration" | "import_statement" | "import_from_statement" | "export_statement"
    )
}

fn is_call_node(kind: &str) -> bool {
    matches!(kind, "call_expression" | "call")
}

fn import_targets(text: &str) -> Vec<String> {
    let stripped = text
        .replace("use ", " ")
        .replace("import ", " ")
        .replace("from ", " ")
        .replace(['\'', '"', ';', '{', '}', '(', ')'], " ");
    stripped
        .split_whitespace()
        .filter(|value| {
            !matches!(
                *value,
                "as" | "*" | "type" | "default" | "export" | "const" | "let" | "var"
            )
        })
        .map(|value| value.trim_matches(',').to_string())
        .filter(|value| !value.is_empty())
        .take(8)
        .collect()
}

fn load_git_history(root: &Path) -> Result<BTreeMap<String, GitFileHistory>> {
    let output = Command::new("git")
        .args([
            "log",
            "--format=@@%H%x09%an%x09%at",
            "--name-only",
            "--no-renames",
            "--",
            ".",
        ])
        .current_dir(root)
        .output()
        .context("run git log for knowledge history")?;
    if !output.status.success() {
        anyhow::bail!(
            "git log failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    let mut result = BTreeMap::<String, GitFileHistory>::new();
    let mut commit = String::new();
    let mut author = String::new();
    let mut timestamp = None;

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some(header) = line.strip_prefix("@@") {
            let mut parts = header.splitn(3, '\t');
            commit = parts.next().unwrap_or("").to_string();
            author = parts.next().unwrap_or("").to_string();
            timestamp = parts.next().and_then(|value| value.parse::<i64>().ok());
            continue;
        }
        let path = line.trim().replace('\\', "/");
        if path.is_empty() || commit.is_empty() {
            continue;
        }
        let history = result.entry(path).or_default();
        history.commits += 1;
        *history.authors.entry(author.clone()).or_insert(0) += 1;
        if history.latest_commit.is_none() {
            history.latest_commit = Some(commit.clone());
            history.latest_unix_timestamp = timestamp;
        }
    }

    Ok(result)
}

fn symbol_id(path: &str, line: usize, kind: &str, name: &str) -> String {
    let raw = format!("{path}:{line}:{kind}:{name}");
    format!("symbol:{}", hex::encode(Sha256::digest(raw.as_bytes())))
}

fn compact_text(value: &str, maximum: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.len() <= maximum {
        return compact;
    }
    let mut end = maximum;
    while !compact.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &compact[..end])
}

fn tokenize(value: &str) -> Vec<String> {
    value
        .split(|character: char| !(character.is_alphanumeric() || character == '_'))
        .filter(|term| term.len() > 1)
        .map(|term| term.to_lowercase())
        .collect()
}

fn cosine_similarity(left: &[f32], right: &[f32]) -> Option<f32> {
    if left.len() != right.len() || left.is_empty() {
        return None;
    }
    let mut dot = 0.0f32;
    let mut left_norm = 0.0f32;
    let mut right_norm = 0.0f32;
    for (a, b) in left.iter().zip(right) {
        dot += a * b;
        left_norm += a * a;
        right_norm += b * b;
    }
    if left_norm <= f32::EPSILON || right_norm <= f32::EPSILON {
        return None;
    }
    Some(dot / (left_norm.sqrt() * right_norm.sqrt()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosine_orders_equal_vectors_at_one() {
        let score = cosine_similarity(&[1.0, 0.0], &[1.0, 0.0]).unwrap();
        assert!((score - 1.0).abs() < 1e-6);
    }

    #[test]
    fn reciprocal_rank_fusion_combines_rankings() {
        let combined = reciprocal_rank_fusion(
            &[vec!["a".into(), "b".into()], vec!["b".into(), "a".into()]],
            2,
        );
        assert_eq!(combined.len(), 2);
    }
}
