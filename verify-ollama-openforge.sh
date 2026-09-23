#!/usr/bin/env bash
set -euo pipefail

REPO="${1:-$PWD}"
CONFIG="$REPO/openforge.yaml"
OLLAMA_URL="${OLLAMA_URL:-http://127.0.0.1:11434}"
DAEMON_PORT="${DAEMON_PORT:-8765}"
DAEMON_URL="http://127.0.0.1:$DAEMON_PORT"

ok() { printf '[ok] %s\n' "$*"; }
err() { printf '[xx] %s\n' "$*" >&2; }

for tool in cargo curl jq git ollama; do
  command -v "$tool" >/dev/null || { err "missing prerequisite: $tool"; exit 1; }
done
[[ -f "$CONFIG" ]] || { err "config not found: $CONFIG"; exit 1; }

curl -sf "$OLLAMA_URL/api/tags" >/dev/null || { err "Ollama is not reachable at $OLLAMA_URL"; exit 1; }
ok "Ollama reachable"

mapfile -t OLLAMA_MODELS < <(ollama list | awk 'NR>1 {print $1}')
mapfile -t DECLARED < <(
  awk '
    /^providers:/ {inp=1; next}
    /^[a-z_]+:/ {if (inp) inp=0}
    inp && /^ *- *id:/ {gsub(/^ *- *id: *| *$/,"",$0); print}
  ' "$CONFIG"
)
(( ${#DECLARED[@]} > 0 )) || { err "no configured models found"; exit 1; }

missing=0
for model in "${DECLARED[@]}"; do
  if printf '%s\n' "${OLLAMA_MODELS[@]}" | grep -Fxq "$model"; then
    ok "installed: $model"
  else
    printf '[!!] configured but not installed: %s (ollama pull %s)\n' "$model" "$model"
    missing=1
  fi
done

(
  cd "$REPO"
  cargo fmt --all --check
  cargo check --locked --workspace --all-targets
  cargo test --locked --workspace
)
ok "Rust workspace verified"

DAEMON_LOG="${TMPDIR:-/tmp}/openforge-ollama-daemon.log"
(
  cd "$REPO"
  cargo run -q -p openforge-daemon -- --config "$CONFIG" --listen "127.0.0.1:$DAEMON_PORT" >"$DAEMON_LOG" 2>&1 &
  echo $!
) >"${TMPDIR:-/tmp}/openforge-ollama-daemon.pid"
DAEMON_PID="$(cat "${TMPDIR:-/tmp}/openforge-ollama-daemon.pid")"
trap 'kill "$DAEMON_PID" 2>/dev/null || true; rm -rf "${VERIFY_REPO:-}"' EXIT

for _ in {1..60}; do
  curl -sf "$DAEMON_URL/health" >/dev/null && break
  sleep .5
done
curl -sf "$DAEMON_URL/health" >/dev/null || { tail -80 "$DAEMON_LOG" >&2; err "daemon did not become healthy"; exit 1; }
ok "OpenForge daemon healthy"

rpc() {
  local id="$1" method="$2" params="$3"
  local payload
  payload="$(jq -nc --argjson id "$id" --arg method "$method" --argjson params "$params"     '{jsonrpc:"2.0",id:$id,method:$method,params:$params}')"
  curl -sf "$DAEMON_URL/v1/rpc"     -H 'content-type: application/json'     --data-binary "$payload"
}

rpc 1 model/list '{}' | jq -e '.result | length > 0' >/dev/null
PREFLIGHT="$(rpc 2 model/preflight '{}')"
echo "$PREFLIGHT" | jq .
echo "$PREFLIGHT" | jq -e '.result | any(.ready == true)' >/dev/null || {
  err "no configured model passed preflight"
  exit 1
}
ok "at least one configured model passed OpenForge preflight"

VERIFY_REPO="$(mktemp -d)"
git -C "$VERIFY_REPO" init -q
printf 'fn main() {}\n' >"$VERIFY_REPO/main.rs"
git -C "$VERIFY_REPO" add main.rs
git -C "$VERIFY_REPO" -c user.name=OpenForge -c user.email=openforge@localhost commit -qm init
RUN="$(rpc 3 run/create "$(jq -nc --arg repo "$VERIFY_REPO" '{repo:$repo,objective:"Ollama verification",autonomy:"suggest",budget_usd:1.0}')")"
RUN_ID="$(echo "$RUN" | jq -er '.result.id')"
COMPLETION="$(rpc 4 completion/request "$(jq -nc --arg run "$RUN_ID" '{run_id:$run,file_path:"main.rs",language:"rust",prefix:"fn ping() -> &'"'"'static str {",suffix:"}",max_output_tokens:32,max_cost_usd:0.0}')")"
echo "$COMPLETION" | jq -e '.result.text | type == "string" and length > 0' >/dev/null
ok "OpenForge fabric completed a live local-model request"

for model in "${DECLARED[@]}"; do
  if ! printf '%s\n' "${OLLAMA_MODELS[@]}" | grep -Fxq "$model"; then
    continue
  fi
  printf 'direct %-28s ' "$model"
  curl -sf "$OLLAMA_URL/api/chat" -H 'content-type: application/json'     -d "$(jq -nc --arg model "$model" '{model:$model,messages:[{role:"user",content:"Reply with PONG"}],stream:false,keep_alive:"1m",options:{num_ctx:2048}}')"     | jq -r '.message.content // .error // "no content"' | head -c 80
  printf '\n'
done

if (( missing )); then
  printf '[!!] verification succeeded for available models; one or more configured models still need ollama pull\n'
fi
ok "Ollama/OpenForge verification complete"
