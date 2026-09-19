use anyhow::{Context, Result};
use ignore::WalkBuilder;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::Path,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SymbolKind {
    Function,
    Method,
    Struct,
    Class,
    Enum,
    Trait,
    Interface,
    TypeAlias,
    Module,
    Constant,
    Variable,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    pub id: String,
    pub name: String,
    pub kind: SymbolKind,
    pub path: String,
    pub line: usize,
    pub signature: String,
    pub language: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Imports,
    References,
    Calls,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct SymbolEdge {
    pub from: String,
    pub to: String,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SymbolGraph {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<SymbolEdge>,
    pub files_indexed: usize,
}

impl SymbolGraph {
    pub fn build(root: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .context("symbol root unavailable")?;
        let mut symbols = Vec::new();
        let mut files = Vec::new();

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
            if fs::metadata(path)?.len() > 2 * 1024 * 1024 {
                continue;
            }

            let content = match fs::read_to_string(path) {
                Ok(content) => content,
                Err(_) => continue,
            };
            let relative = path
                .strip_prefix(&root)?
                .to_string_lossy()
                .replace('\\', "/");
            let language = language_for(path);
            if language == "text" {
                continue;
            }

            let file_symbols = extract_symbols(&relative, &language, &content);
            files.push((relative, language, content));
            symbols.extend(file_symbols);
        }

        symbols.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.line.cmp(&right.line))
                .then_with(|| left.name.cmp(&right.name))
        });

        let by_name: HashMap<String, Vec<String>> = {
            let mut map: HashMap<String, Vec<String>> = HashMap::new();
            for symbol in &symbols {
                map.entry(symbol.name.clone())
                    .or_default()
                    .push(symbol.id.clone());
            }
            map
        };

        let mut edges = BTreeSet::new();
        for (path, language, content) in &files {
            let source_ids: Vec<String> = symbols
                .iter()
                .filter(|symbol| &symbol.path == path)
                .map(|symbol| symbol.id.clone())
                .collect();

            for imported in extract_imports(language, content) {
                let import_target = format!("module:{imported}");
                for source in &source_ids {
                    edges.insert(SymbolEdge {
                        from: source.clone(),
                        to: import_target.clone(),
                        kind: EdgeKind::Imports,
                    });
                }
            }

            for (name, targets) in &by_name {
                if !contains_identifier(content, name) {
                    continue;
                }
                let call_like = contains_call(content, name);
                for source in &source_ids {
                    for target in targets {
                        if source == target {
                            continue;
                        }
                        edges.insert(SymbolEdge {
                            from: source.clone(),
                            to: target.clone(),
                            kind: if call_like {
                                EdgeKind::Calls
                            } else {
                                EdgeKind::References
                            },
                        });
                    }
                }
            }
        }

        Ok(Self {
            files_indexed: files.len(),
            symbols,
            edges: edges.into_iter().collect(),
        })
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Symbol> {
        let terms: Vec<String> = query
            .split(|character: char| !character.is_alphanumeric() && character != '_')
            .filter(|term| !term.is_empty())
            .map(|term| term.to_lowercase())
            .collect();

        let mut ranked: Vec<(i32, &Symbol)> = self
            .symbols
            .iter()
            .filter_map(|symbol| {
                let name = symbol.name.to_lowercase();
                let path = symbol.path.to_lowercase();
                let mut score = 0;
                for term in &terms {
                    if name == *term {
                        score += 20;
                    } else if name.starts_with(term) {
                        score += 12;
                    } else if name.contains(term) {
                        score += 8;
                    }
                    if path.contains(term) {
                        score += 3;
                    }
                }
                (score > 0).then_some((score, symbol))
            })
            .collect();

        ranked.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.path.cmp(&right.1.path))
                .then_with(|| left.1.line.cmp(&right.1.line))
        });

        ranked
            .into_iter()
            .take(limit.min(1000))
            .map(|(_, symbol)| symbol.clone())
            .collect()
    }

    pub fn stats(&self) -> BTreeMap<String, usize> {
        let mut result = BTreeMap::from([
            ("files".into(), self.files_indexed),
            ("symbols".into(), self.symbols.len()),
            ("edges".into(), self.edges.len()),
        ]);
        for symbol in &self.symbols {
            *result
                .entry(format!("kind.{:?}", symbol.kind).to_lowercase())
                .or_insert(0) += 1;
        }
        result
    }
}

fn extract_symbols(path: &str, language: &str, content: &str) -> Vec<Symbol> {
    let mut symbols = Vec::new();

    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("//") || trimmed.starts_with('#') {
            continue;
        }

        if let Some((kind, name)) = declaration(language, trimmed) {
            symbols.push(Symbol {
                id: format!("{path}:{}:{name}", index + 1),
                name,
                kind,
                path: path.to_string(),
                line: index + 1,
                signature: trimmed.to_string(),
                language: language.to_string(),
            });
        }
    }

    symbols
}

fn declaration(language: &str, line: &str) -> Option<(SymbolKind, String)> {
    match language {
        "rust" => declaration_by_prefix(
            line,
            &[
                ("pub async fn ", SymbolKind::Function),
                ("async fn ", SymbolKind::Function),
                ("pub fn ", SymbolKind::Function),
                ("fn ", SymbolKind::Function),
                ("pub struct ", SymbolKind::Struct),
                ("struct ", SymbolKind::Struct),
                ("pub enum ", SymbolKind::Enum),
                ("enum ", SymbolKind::Enum),
                ("pub trait ", SymbolKind::Trait),
                ("trait ", SymbolKind::Trait),
                ("pub type ", SymbolKind::TypeAlias),
                ("type ", SymbolKind::TypeAlias),
                ("pub const ", SymbolKind::Constant),
                ("const ", SymbolKind::Constant),
                ("mod ", SymbolKind::Module),
                ("pub mod ", SymbolKind::Module),
            ],
        ),
        "typescript" | "javascript" => declaration_by_prefix(
            normalize_export(line),
            &[
                ("async function ", SymbolKind::Function),
                ("function ", SymbolKind::Function),
                ("class ", SymbolKind::Class),
                ("interface ", SymbolKind::Interface),
                ("type ", SymbolKind::TypeAlias),
                ("const ", SymbolKind::Constant),
                ("let ", SymbolKind::Variable),
            ],
        ),
        "python" => declaration_by_prefix(
            line,
            &[
                ("async def ", SymbolKind::Function),
                ("def ", SymbolKind::Function),
                ("class ", SymbolKind::Class),
            ],
        ),
        "go" => declaration_by_prefix(
            line,
            &[
                ("func ", SymbolKind::Function),
                ("type ", SymbolKind::TypeAlias),
                ("const ", SymbolKind::Constant),
                ("var ", SymbolKind::Variable),
            ],
        ),
        "java" | "kotlin" => {
            let words: Vec<&str> = line.split_whitespace().collect();
            if let Some(position) = words
                .iter()
                .position(|word| matches!(*word, "class" | "interface" | "enum"))
            {
                let name = words
                    .get(position + 1)?
                    .trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
                let kind = match words[position] {
                    "class" => SymbolKind::Class,
                    "interface" => SymbolKind::Interface,
                    _ => SymbolKind::Enum,
                };
                (!name.is_empty()).then_some((kind, name.to_string()))
            } else if line.contains('(') && line.contains(')') && line.ends_with('{') {
                let before = line.split('(').next()?.trim();
                let name = before.split_whitespace().last()?;
                Some((
                    SymbolKind::Method,
                    name.trim_matches(|c: char| !c.is_alphanumeric() && c != '_')
                        .to_string(),
                ))
            } else {
                None
            }
        }
        _ => None,
    }
}

fn declaration_by_prefix(
    line: &str,
    prefixes: &[(&str, SymbolKind)],
) -> Option<(SymbolKind, String)> {
    for (prefix, kind) in prefixes {
        if let Some(rest) = line.strip_prefix(prefix) {
            let name = rest
                .split(|character: char| !(character.is_alphanumeric() || character == '_'))
                .next()
                .unwrap_or("");
            if !name.is_empty() {
                return Some((kind.clone(), name.to_string()));
            }
        }
    }
    None
}

fn normalize_export(line: &str) -> &str {
    line.strip_prefix("export default ")
        .or_else(|| line.strip_prefix("export "))
        .unwrap_or(line)
}

fn extract_imports(language: &str, content: &str) -> BTreeSet<String> {
    let mut imports = BTreeSet::new();

    for line in content.lines() {
        let trimmed = line.trim();
        let candidate = match language {
            "rust" => trimmed
                .strip_prefix("use ")
                .or_else(|| trimmed.strip_prefix("pub use "))
                .map(|value| value.trim_end_matches(';')),
            "python" => trimmed
                .strip_prefix("import ")
                .or_else(|| trimmed.strip_prefix("from "))
                .map(|value| value.split_whitespace().next().unwrap_or(value)),
            "typescript" | "javascript" => {
                if trimmed.starts_with("import ") {
                    trimmed
                        .split(" from ")
                        .nth(1)
                        .or_else(|| trimmed.split_whitespace().nth(1))
                        .map(|value| value.trim_matches(|c| c == '\'' || c == '"' || c == ';'))
                } else {
                    None
                }
            }
            "go" | "java" | "kotlin" => trimmed
                .strip_prefix("import ")
                .map(|value| value.trim_matches(|c| c == '\'' || c == '"' || c == ';')),
            _ => None,
        };

        if let Some(candidate) = candidate {
            if !candidate.is_empty() {
                imports.insert(candidate.to_string());
            }
        }
    }

    imports
}

fn contains_identifier(content: &str, identifier: &str) -> bool {
    content.match_indices(identifier).any(|(index, _)| {
        let left = content[..index].chars().next_back();
        let right = content[index + identifier.len()..].chars().next();
        left.is_none_or(|character| !is_identifier_char(character))
            && right.is_none_or(|character| !is_identifier_char(character))
    })
}

fn contains_call(content: &str, identifier: &str) -> bool {
    let needle = format!("{identifier}(");
    content.contains(&needle)
}

fn is_identifier_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
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
        _ => "text",
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_rust_declarations() {
        let symbols = extract_symbols(
            "src/lib.rs",
            "rust",
            "pub struct Engine {}\nimpl Engine {}\npub fn run() {}",
        );
        assert_eq!(symbols.len(), 2);
        assert_eq!(symbols[0].name, "Engine");
        assert_eq!(symbols[1].name, "run");
    }
}
