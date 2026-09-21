import { useEffect, useState } from "react";

interface GatewayStatus {
  connected: boolean;
  base_url: string;
  model: string;
  credential_storage: string;
  pricing: string;
}
interface GatewayClient {
  gatewayStatus(): Promise<GatewayStatus>;
  connectGateway(apiKey: string): Promise<GatewayStatus>;
  disconnectGateway(): Promise<GatewayStatus>;
}

/** Provider keys are never placed in browser storage or read back from the daemon. */
export function GatewaySettings({ client }: { client: GatewayClient }) {
  const [status, setStatus] = useState<GatewayStatus | null>(null);
  const [apiKey, setApiKey] = useState("");
  const [visible, setVisible] = useState(false);
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  const [error, setError] = useState("");

  useEffect(() => {
    let active = true;
    setStatus(null);
    void client.gatewayStatus().then((value) => {
      if (active) { setStatus(value); setError(""); }
    }).catch((failure: unknown) => {
      if (active) setError(failure instanceof Error ? failure.message : String(failure));
    });
    return () => { active = false; };
  }, [client]);

  async function connect() {
    setBusy(true);
    setError("");
    setMessage("Testing a short request with Sevi…");
    try {
      const result = await client.connectGateway(apiKey.trim());
      setStatus(result);
      setApiKey("");
      setVisible(false);
      setMessage("Connected. New planning, agent and completion requests use Sevi.");
    } catch (failure) {
      setMessage("");
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally { setBusy(false); }
  }

  async function disconnect() {
    setBusy(true);
    setError("");
    try {
      setStatus(await client.disconnectGateway());
      setApiKey("");
      setVisible(false);
      setMessage("Disconnected. New requests use your original provider configuration.");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    } finally { setBusy(false); }
  }

  return (
    <section aria-label="Sevi model gateway" className="gateway-settings">
      <h2>Sevi model gateway</h2>
      <p>Use your Sevi account directly in OpenForge. No Cursor subscription is needed.</p>
      <dl>
        <dt>Gateway</dt><dd>https://model.sevi.io/cursor</dd>
        <dt>Model</dt><dd>auto-select</dd>
        <dt>Status</dt><dd>{status?.connected ? "Connected for this daemon session" : "Not connected"}</dd>
      </dl>
      <form onSubmit={(event) => { event.preventDefault(); void connect(); }}>
        <label>
          Sevi API key
          <input
            aria-label="Sevi API key"
            type={visible ? "text" : "password"}
            value={apiKey}
            onChange={(event) => setApiKey(event.target.value)}
            autoComplete="off"
            autoCapitalize="none"
            spellCheck={false}
            placeholder="Paste your actual gateway key"
            disabled={busy}
            required
          />
        </label>
        <div className="gateway-actions">
          <button type="button" aria-pressed={visible} onClick={() => setVisible(!visible)} disabled={busy}>{visible ? "Hide key" : "Show key"}</button>
          <button type="submit" disabled={busy || !apiKey.trim()}>{busy ? "Working…" : "Test & connect"}</button>
          {status?.connected && <button type="button" disabled={busy} onClick={() => void disconnect()}>Disconnect Sevi</button>}
        </div>
      </form>
      <p>The key stays in daemon memory only. Re-enter it after restarting the daemon. Disconnect affects new requests; requests already running may finish.</p>
      <p>Testing sends one short request. Sevi controls model access, quotas and any charges. USD totals are unpriced unless the gateway reports a cost; a zero display does not guarantee free usage.</p>
      {message && <p role="status">{message}</p>}
      {error && <p role="alert">{error}</p>}
    </section>
  );
}
