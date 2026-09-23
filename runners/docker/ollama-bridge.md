# Docker runner → host Ollama bridge

OpenForge's control plane normally calls the configured model provider from the host daemon, so ordinary agent execution does not require Ollama inside the sandbox. The bridge in `compose.ollama.yaml` is for runner tasks, diagnostics, or tools that intentionally need to reach the host Ollama API.

Start Ollama on the host and make it reachable from the Docker bridge. On Linux, Ollama commonly binds only to `127.0.0.1`; `host.docker.internal` cannot reach that loopback-only listener. Bind Ollama to a host interface deliberately and protect port `11434` with the host firewall. Do not expose the unauthenticated Ollama API to an untrusted network.

```bash
OLLAMA_HOST=0.0.0.0:11434 ollama serve

docker compose -f runners/docker/compose.ollama.yaml run --rm runner \
  curl -fsS http://host.docker.internal:11434/api/tags
```

The compose file adds the Linux `host-gateway` mapping and sets `OLLAMA_HOST=http://host.docker.internal:11434` inside the runner. macOS and Windows Docker Desktop already provide `host.docker.internal`.

If host networking is preferable on Linux, run the runner with `--network host` and keep Ollama bound to loopback. Do not combine host networking with untrusted autonomous workloads unless the policy and containment boundary explicitly allow access to host-local services.
