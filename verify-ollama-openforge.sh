#!/usr/bin/env bash
# End-to-end verification of OpenForge's configured local Ollama fabric.
set -euo pipefail

REPO="${1:-$(pwd)}"
CONFIG="$REPO/openforge.yaml"
OLLAMA_URL="${OLLAMA_URL:-http://127.0.0.1:11434}"
DAEMON_PORT="${DAEMON_PORT:-8765}"
TEST_MODEL="${TEST_MODEL:-qwen2.5-coder:7b}"

G='\033[0;32m'; Y='\033[1;33m'; R='\033[0;31m'; C='\033[0;36m'; N='\033[0m'
ok() { printf '%b[ok]%b %s\n' "$G" "$N" "$*"; }
warn() { printf '%b[!!]%b %s\n' "$Y" "$N" "$*"; }
err() { printf '%b[xx]%b %s\n' "$R" "$N" "$*" >&2; }
hd() { printf '%b== %s ==%b\n' "$C" "$*" "$N"; }

hd "Prerequisites"
for tool in cargo jq curl ollama awk sed; do
  command -v "$tool" >/dev/null || { err "missing prerequisite: $tool"; exit 1; }
  ok "$tool"
done
[[ -d "$REPO" ]] || { err "repo not found: $REPO"; exit 1; }
[[ -f "$CONFIG" ]] || { err "config not found: $CONFIG"; exit 1; }

hd "Ollama service"
curl -fsS "$OLLAMA_URL/api/tags" >/dev/null || {
  err "Ollama is not reachable at $OLLAMA_URL"
  exit 1
}
ok "Ollama reachable"
mapfile -t OLLAMA_MODELS < <(ollama list | awk 'NR > 1 {print $1}')
printf '  %s\n' "${OLLAMA_MODELS[@]}"

hd "Configured local models"
mapfile -t DECLARED < <(awk '/^      - id: / {sub(/^      - id: /, ""); print}' "$CONFIG")
(( ${#DECLARED[@]} > 0 )) || { err "no configured model ids found in $CONFIG"; exit 1; }
printf '  %s\n' "${DECLARED[@]}"

missing=0
for model in "${DECLARED[@]}"; do
  if printf '%s\n' "${OLLAMA_MODELS[@]}" | grep -Fxq "$model"; then
    ok "installed: $model"
  else
    warn "configured but not installed: $model (ollama pull $model)"
    missing=$((missing + 1))
  fi
done
if ! printf '%s\n' "${OLLAMA_MODELS[@]}" | grep -Fxq "$TEST_MODEL"; then
  err "TEST_MODEL=$TEST_MODEL is not installed; choose an installed configured model"
  exit 1
fi

hd "Workspace build"
(
  cd "$REPO"
  cargo build --locked --workspace --all-targets
)
ok "workspace builds"

hd "Isolated daemon catalog"
TMP="$(mktemp -d)"
DAEMON_PID=""
cleanup() {
  if [[ -n "$DAEMON_PID" ]]; then
    kill "$DAEMON_PID" 2>/dev/null || true
    wait "$DAEMON_PID" 2>/dev/null || true
  fi
  rm -rf "$TMP"
}
trap cleanup EXIT
cp "$CONFIG" "$TMP/openforge.yaml"
sed -i \
  -e "s#^state_db:.*#state_db: $TMP/state.db#" \
  -e "s#^artifact_dir:.*#artifact_dir: $TMP/artifacts#" \
  -e "s#^worktree_dir:.*#worktree_dir: $TMP/worktrees#" \
  "$TMP/openforge.yaml"
(
  cd "$REPO"
  cargo run -q -p openforge-daemon -- \
    --config "$TMP/openforge.yaml" \
    --listen "127.0.0.1:$DAEMON_PORT" \
    >"$TMP/daemon.log" 2>&1
) &
DAEMON_PID=$!
for _ in {1..60}; do
  curl -fsS "http://127.0.0.1:$DAEMON_PORT/health" >/dev/null 2>&1 && break
  if ! kill -0 "$DAEMON_PID" 2>/dev/null; then
    err "daemon exited during startup"
    cat "$TMP/daemon.log" >&2
    exit 1
  fi
  sleep 0.5
done
curl -fsS "http://127.0.0.1:$DAEMON_PORT/health" >/dev/null || {
  err "daemon did not become healthy"
  tail -80 "$TMP/daemon.log" >&2
  exit 1
}
ok "daemon healthy"

rpc() {
  local method="$1" params="$2"
  curl -fsS "http://127.0.0.1:$DAEMON_PORT/v1/rpc" \
    -H 'Content-Type: application/json' \
    -d "$(jq -cn --arg method "$method" --argjson params "$params" \
      '{jsonrpc:"2.0",id:1,method:$method,params:$params}')"
}

CATALOG="$(rpc model/list '{}')"
echo "$CATALOG" | jq -e '.error == null' >/dev/null
for model in "${DECLARED[@]}"; do
  echo "$CATALOG" | jq -e --arg model "$model" \
    '.result | any(.provider == "local" and .model == $model and .input_usd_per_million == 0 and .output_usd_per_million == 0)' \
    >/dev/null || { err "daemon model/list missing or mispriced: $model"; exit 1; }
done
echo "$CATALOG" | jq '.result | map({provider, model, supports_tools, supports_structured_output, context_tokens})'
ok "daemon catalog matches configured local models"

hd "Ollama MCP inventory"
MCP="$(rpc mcp/call_tool '{"server_name":"ollama","tool_name":"list_models","arguments":{},"policy_path":"config/policies/development.yaml"}')"
echo "$MCP" | jq -e '.error == null and .result.isError == false' >/dev/null || {
  err "Ollama MCP inventory failed"
  echo "$MCP" | jq . >&2
  exit 1
}
echo "$MCP" | jq '.result.structuredContent // .result.content'
ok "policy-gated Ollama MCP tool works"

hd "Fabric native invocation"
(
  cd "$REPO"
  OLLAMA_URL="$OLLAMA_URL" cargo run -q -p openforge-models --example ollama_probe -- "$TEST_MODEL"
)
ok "FabricProvider invoked $TEST_MODEL through the local adapter"

hd "Direct Ollama checks"
for model in "${DECLARED[@]}"; do
  if ! printf '%s\n' "${OLLAMA_MODELS[@]}" | grep -Fxq "$model"; then
    continue
  fi
  printf '  %-28s ' "$model"
  started="$(date +%s%N)"
  result="$(curl -fsS "$OLLAMA_URL/v1/chat/completions" \
    -H 'Content-Type: application/json' \
    -d "$(jq -cn --arg model "$model" '{model:$model,messages:[{role:"user",content:"hi"}],stream:false,max_tokens:4}')" \
    | jq -r '.choices[0].message.content // .error.message // "ERR"' 2>/dev/null || printf 'ERR')"
  ended="$(date +%s%N)"
  printf '%sms  %s\n' "$(( (ended - started) / 1000000 ))" "${result:0:60}"
done

printf '\n'
ok "verification complete ($missing configured model(s) not installed)"
