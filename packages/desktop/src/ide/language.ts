export function languageForPath(path: string): string {
  const lower = path.toLowerCase();
  if (lower.endsWith(".tsx")) return "typescriptreact";
  if (lower.endsWith(".ts")) return "typescript";
  if (lower.endsWith(".jsx")) return "javascriptreact";
  if (lower.endsWith(".js") || lower.endsWith(".mjs") || lower.endsWith(".cjs")) {
    return "javascript";
  }
  if (lower.endsWith(".rs")) return "rust";
  if (lower.endsWith(".py")) return "python";
  if (lower.endsWith(".go")) return "go";
  if (lower.endsWith(".java")) return "java";
  if (lower.endsWith(".kt") || lower.endsWith(".kts")) return "kotlin";
  if (lower.endsWith(".cs")) return "csharp";
  if (lower.endsWith(".cpp") || lower.endsWith(".cc") || lower.endsWith(".cxx")) {
    return "cpp";
  }
  if (lower.endsWith(".c") || lower.endsWith(".h")) return "c";
  if (lower.endsWith(".json") || lower.endsWith(".jsonc")) return "json";
  if (lower.endsWith(".yaml") || lower.endsWith(".yml")) return "yaml";
  if (lower.endsWith(".toml")) return "toml";
  if (lower.endsWith(".md") || lower.endsWith(".mdx")) return "markdown";
  if (lower.endsWith(".html") || lower.endsWith(".htm")) return "html";
  if (lower.endsWith(".css")) return "css";
  if (lower.endsWith(".scss")) return "scss";
  if (lower.endsWith(".sql")) return "sql";
  if (lower.endsWith(".sh") || lower.endsWith(".bash") || lower.endsWith(".zsh")) {
    return "shell";
  }
  if (lower.endsWith(".xml")) return "xml";
  return "plaintext";
}

export function lspLanguageId(monacoLanguage: string): string {
  switch (monacoLanguage) {
    case "typescriptreact":
      return "typescriptreact";
    case "javascriptreact":
      return "javascriptreact";
    case "shell":
      return "shellscript";
    default:
      return monacoLanguage;
  }
}

export function fileUri(repo: string, relative: string): string {
  const root = repo.replaceAll("\\", "/").replace(/\/$/, "");
  const path = relative.replaceAll("\\", "/").replace(/^\//, "");
  const full = root + "/" + path;
  return full.startsWith("/")
    ? "file://" + encodeURI(full)
    : "file:///" + encodeURI(full);
}

export function basename(path: string): string {
  const normalized = path.replaceAll("\\", "/");
  return normalized.slice(normalized.lastIndexOf("/") + 1);
}
