# GitHub and automation extensions

OpenForge uses MCP stdio servers as automation extensions. The checked-in configuration enables a bundled GitHub connector using the GitHub CLI's existing authentication. Install GitHub CLI if it is not already available, then run `gh auth login --hostname github.com`. OpenForge never copies the GitHub credential into YAML or model prompts. GitHub authentication is separate from the Sevi model key.

After setup/restarting the daemon:

```bash
openforge mcp servers
openforge mcp tools github
openforge mcp call github get_repository '{"owner":"GAN-007","repo":"OpenForge"}'
openforge mcp call github list_issues '{"owner":"GAN-007","repo":"OpenForge","page":1}'
```

The connector supports repository details, paginated issues, pull requests and workflow runs, plus issue creation, pull-request creation and workflow dispatch. All API requests target github.com, use argument arrays without a shell, and have a 60-second timeout. Lists return 30 results per page; increment `page` to continue. Failed calls return a sanitized error without credentials. The implementation follows [GitHub CLI API handling](https://cli.github.com/manual/gh_api) and [MCP tool discovery and calls](https://modelcontextprotocol.io/specification/2025-06-18/server/tools).

The development policy permits GitHub discovery and the four read operations. Mutating operations default to `ask` and therefore return an approval-required error until explicitly allowed by the operator's policy. They are not silently approved by connecting GitHub. Repository permissions and service limits still apply. No issue, PR or workflow is created during setup or discovery.

## Additional MCP servers

Install the desired server using its publisher's instructions. Register its executable, arguments, optional working directory, and timeout in `mcp_servers` in the daemon's `openforge.yaml`. Each server needs a unique name. Keep credentials in the service's credential store rather than committed YAML. MCP processes receive PATH and explicitly configured environment fields only; daemon credentials are not automatically inherited. The bundled GitHub server resolves the current user's home directory to locate gh's credential store. Restart the daemon after configuration changes.

Authorize the exact `<server>/tools/list` capability and required `<server>/<tool>` capabilities in the selected policy's `mcp.allow` list. `openforge mcp servers` lists configured names without disclosing environment variables; `openforge mcp tools SERVER` obtains the server's schemas. `openforge mcp call SERVER TOOL 'JSON'` invokes a tool using daemon policy checks. The agent obtains schemas for allowed tools before execution and invokes them through the same policy-protected tool bus. Discovery failure does not prevent ordinary file tasks.

Only stdio MCP servers are supported here; remote HTTP-only extensions need a separately configured bridge. Extensions requiring their own accounts or paid services retain those requirements. This integration does not grant access to another application's proprietary plugins.
