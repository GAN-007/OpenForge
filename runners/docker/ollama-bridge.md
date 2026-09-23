# Ollama access from Docker runners

The OpenForge model fabric runs in the daemon process, so normal model calls do not need to cross the sandbox boundary. This bridge is for acceptance tests, tools, or repository code executed inside the Docker runner that also need to call the host Ollama API.

Start Ollama so it accepts the Docker bridge connection, then launch the runner with:

```bash
docker compose -f runners/docker/docker-compose.ollama.yml run --rm openforge-runner
```

Inside the container, use `$OLLAMA_URL` (configured as `http://host.docker.internal:11434`). The compose file adds Docker's Linux `host-gateway` mapping and does not mount Docker's control socket or expose unrelated host services.

If Ollama is intentionally bound only to `127.0.0.1`, container access will fail. Keep it loopback-only when containers do not need direct Ollama access; otherwise bind Ollama to an interface reachable from Docker and use host firewall rules to restrict access to the local bridge.
