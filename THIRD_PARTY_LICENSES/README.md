# Third-party license inventory

OpenForge is an original implementation informed by public projects and standards. The repository does not intentionally vendor source files from OpenCode, OpenHands, Goose, Kilo Code, Cline or Aider in this initial implementation.

Runtime/build dependencies are resolved through Cargo, pnpm, Python and Gradle package managers and retain their upstream licenses. Release CI is expected to generate an SBOM so the exact transitive dependency inventory matches the released commit.

Architectural references that influenced the design include:

| Project / standard | Relevant upstream license or status | Use in OpenForge |
| --- | --- | --- |
| OpenCode | MIT | architectural reference; optional interoperability target |
| OpenHands open core / SDK | MIT | architectural reference; optional adapter target |
| Goose | Apache-2.0 ecosystem | MCP/ACP and automation reference |
| Kilo Code core | MIT | IDE/completion/browser reference |
| Cline core/SDK | Apache-2.0 | agent/runtime/worktree reference |
| Aider | Apache-2.0 | Git/repository-map reference |
| Model Context Protocol | open protocol / SDK-specific licenses | tool interoperability |
| Agent Client Protocol | Apache-licensed protocol ecosystem | editor-agent interoperability |

This file is an engineering inventory, not legal advice. Before redistributing copied or vendored upstream source, verify the exact file-level license and preserve all required copyright and NOTICE material.
