import { useEffect, useRef, useState } from "react";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import type { OpenForgeClient, TerminalDescriptor, TerminalEvent } from "@openforge/sdk";

export function TerminalPane(props: {
  client: OpenForgeClient;
  repo: string;
  platform: string;
  initialCommand?: string;
}) {
  const { client, repo, platform, initialCommand = "" } = props;
  const hostRef = useRef<HTMLDivElement | null>(null);
  const [descriptor, setDescriptor] = useState<TerminalDescriptor | null>(null);
  const [status, setStatus] = useState("starting");

  useEffect(() => {
    const host = hostRef.current;
    if (!host || !repo) return;

    const terminal = new Terminal({
      cursorBlink: true,
      convertEol: false,
      fontFamily:
        "JetBrains Mono, SFMono-Regular, Consolas, Liberation Mono, monospace",
      fontSize: 13,
      scrollback: 10_000,
      theme: {
        background: "#0a0d12",
        foreground: "#dce2ef",
        cursor: "#8b7cff",
        selectionBackground: "#383163",
      },
    });
    const fit = new FitAddon();
    terminal.loadAddon(fit);
    terminal.open(host);
    fit.fit();

    let disposed = false;
    let socket: WebSocket | undefined;
    let current: TerminalDescriptor | undefined;
    const program = platform === "windows" ? "powershell.exe" : "bash";

    const dataDisposable = terminal.onData((data) => {
      if (socket?.readyState === WebSocket.OPEN) {
        socket.send(data);
      }
    });

    const resize = async () => {
      fit.fit();
      if (current) {
        try {
          await client.resizeTerminal(current.id, terminal.rows, terminal.cols);
        } catch {
          // Resize is best-effort; the PTY remains usable at its previous size.
        }
      }
    };
    const observer = new ResizeObserver(() => void resize());
    observer.observe(host);

    void client
      .spawnTerminal({
        repo,
        program,
        cwd: ".",
        rows: terminal.rows,
        cols: terminal.cols,
      })
      .then((created) => {
        if (disposed) {
          void client.closeTerminal(created.id);
          return;
        }
        current = created;
        setDescriptor(created);
        socket = client.terminalSocket(created.id);
        socket.onopen = () => {
          setStatus("connected");
          if (initialCommand.trim()) {
            socket?.send(initialCommand.trim() + "\r");
          }
        };
        socket.onmessage = (message) => {
          try {
            const event = JSON.parse(String(message.data)) as TerminalEvent;
            if (event.data) terminal.write(event.data);
          } catch {
            terminal.write(String(message.data));
          }
        };
        socket.onerror = () => setStatus("error");
        socket.onclose = () => setStatus("closed");
      })
      .catch((error: unknown) => {
        setStatus(error instanceof Error ? error.message : String(error));
      });

    return () => {
      disposed = true;
      observer.disconnect();
      dataDisposable.dispose();
      socket?.close();
      terminal.dispose();
      if (current) void client.closeTerminal(current.id);
    };
  }, [client, initialCommand, platform, repo]);

  return (
    <section className="terminal-pane">
      <div className="pane-toolbar">
        <span>Terminal</span>
        <span className="pane-status">
          {descriptor ? descriptor.program : status}
        </span>
      </div>
      <div ref={hostRef} className="xterm-host" />
    </section>
  );
}
