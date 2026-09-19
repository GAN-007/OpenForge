import { useState } from "react";
import type { OpenForgeClient } from "@openforge/sdk";

type BrowserView = "screenshot" | "network" | "console" | "accessibility";

export function BrowserPane(props: { client: OpenForgeClient }) {
  const { client } = props;
  const [url, setUrl] = useState("http://localhost:3000");
  const [title, setTitle] = useState("");
  const [view, setView] = useState<BrowserView>("screenshot");
  const [screenshot, setScreenshot] = useState("");
  const [data, setData] = useState<unknown>(null);
  const [error, setError] = useState("");

  async function navigate() {
    try {
      const result = await client.browserNavigate(url);
      setTitle(result.title);
      setUrl(result.url);
      await capture();
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function capture() {
    try {
      const result = await client.browserScreenshot(true);
      setScreenshot("data:image/png;base64," + result.base64);
      setView("screenshot");
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function network() {
    try {
      setData(await client.browserNetworkEntries(2000));
      setView("network");
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function consoleEntries() {
    try {
      setData(await client.browserConsole(2000));
      setView("console");
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  async function accessibility() {
    try {
      setData(await client.browserAccessibilitySnapshot("body"));
      setView("accessibility");
      setError("");
    } catch (failure) {
      setError(failure instanceof Error ? failure.message : String(failure));
    }
  }

  return (
    <section className="browser-pane">
      <div className="pane-toolbar">
        <span>Browser QA</span>
        <span className="pane-status">{title || "headless Playwright"}</span>
      </div>
      <div className="browser-address">
        <input
          value={url}
          onChange={(event) => setUrl(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") void navigate();
          }}
        />
        <button onClick={() => void navigate()}>Go</button>
        <button onClick={() => void capture()}>Screenshot</button>
        <button onClick={() => void network()}>Network</button>
        <button onClick={() => void consoleEntries()}>Console</button>
        <button onClick={() => void accessibility()}>Accessibility</button>
      </div>
      {error && <div className="pane-error">{error}</div>}
      <div className="browser-output">
        {view === "screenshot" && screenshot ? (
          <img src={screenshot} alt={"Browser capture of " + url} />
        ) : view === "screenshot" ? (
          <div className="pane-empty">Navigate to an application to collect visual evidence.</div>
        ) : (
          <pre>{JSON.stringify(data, null, 2)}</pre>
        )}
      </div>
    </section>
  );
}
