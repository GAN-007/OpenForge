import {useEffect,useMemo,useState} from "react";
import {invoke} from "@tauri-apps/api/core";
import {OpenForgeClient,type CapabilitySet} from "@openforge/sdk";
import {Panel} from "@openforge/ui";

export function DesktopApp(){
  const client=useMemo(()=>new OpenForgeClient(),[]);
  const [caps,setCaps]=useState<CapabilitySet|null>(null);
  const [daemon,setDaemon]=useState("checking");
  const [repo,setRepo]=useState("");
  const [system,setSystem]=useState<{platform:string;arch:string}|null>(null);
  useEffect(()=>{void client.initialize().then(v=>{setCaps(v);setDaemon("online")}).catch(()=>setDaemon("offline"));void invoke<{platform:string;arch:string}>("system_info").then(setSystem)},[client]);
  return <main className="desktop">
    <header><div><h1>OpenForge</h1><p>Engineering OS</p></div><div className={`daemon daemon--${daemon}`}>{daemon}</div></header>
    <nav><button>Workspace</button><button>Agents</button><button>Graph</button><button>Diff</button><button>Browser</button><button>Database</button><button>Logs</button><button>Cost</button><button>Context</button><button>History</button></nav>
    <section className="workspace">
      <Panel title="Repository"><input value={repo} onChange={e=>setRepo(e.target.value)} placeholder="/path/to/repository"/><p>Repository operations are executed by the OpenForge daemon and policy broker.</p></Panel>
      <Panel title="Control Plane"><dl><dt>Protocol</dt><dd>{caps?.protocol_version??"unavailable"}</dd><dt>Daemon</dt><dd>{caps?.server_version??"unavailable"}</dd><dt>Host</dt><dd>{system?`${system.platform}/${system.arch}`:"loading"}</dd></dl></Panel>
      <Panel title="Capabilities" className="wide"><div className="capabilities">{Object.entries(caps?.capabilities??{}).map(([k,v])=><span key={k} className={v?"enabled":""}>{k}</span>)}</div></Panel>
    </section>
  </main>
}
