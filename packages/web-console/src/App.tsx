import {useCallback,useEffect,useMemo,useState} from "react";
import {OpenForgeClient,type EventEnvelope,type Run,type TaskNode} from "@openforge/sdk";
import {Panel,StatusPill,EmptyState} from "@openforge/ui";

export function App(){
  const client=useMemo(()=>new OpenForgeClient(),[]);
  const [online,setOnline]=useState(false);
  const [runId,setRunId]=useState("");
  const [run,setRun]=useState<Run|null>(null);
  const [tasks,setTasks]=useState<TaskNode[]>([]);
  const [events,setEvents]=useState<EventEnvelope[]>([]);
  const [cost,setCost]=useState(0);
  const [error,setError]=useState("");

  const refresh=useCallback(async()=>{
    setOnline(await client.health());
    if(!runId.trim()) return;
    try{
      const [r,t,e,b]=await Promise.all([client.getRun(runId.trim()),client.listTasks(runId.trim()),client.listEvents(runId.trim()),client.getBudget(runId.trim())]);
      setRun(r);setTasks(t);setEvents(e);setCost(b.spent_usd);setError("");
    }catch(err){setError(err instanceof Error?err.message:String(err))}
  },[client,runId]);

  useEffect(()=>{void refresh();const id=window.setInterval(()=>void refresh(),3000);return()=>window.clearInterval(id)},[refresh]);

  return <main className="shell">
    <header className="topbar">
      <div><h1>OpenForge</h1><p>AI Engineering Control Plane</p></div>
      <div className="connection"><span className={online?"dot dot--ok":"dot"}/>{online?"daemon online":"daemon offline"}</div>
    </header>
    <div className="toolbar">
      <input aria-label="Run ID" value={runId} onChange={e=>setRunId(e.target.value)} placeholder="Paste a run UUID"/>
      <button onClick={()=>void refresh()}>Refresh</button>
      {run&&<><StatusPill status={run.status}/><span className="cost">${cost.toFixed(4)}</span></>}
    </div>
    {error&&<div className="error">{error}</div>}
    <div className="grid">
      <Panel title="Objective" className="objective">
        {run?<><h2>{run.objective}</h2><dl><dt>Base SHA</dt><dd>{run.base_sha}</dd><dt>Autonomy</dt><dd>{run.autonomy}</dd><dt>Budget</dt><dd>${run.budget.hard_limit.toFixed(2)}</dd></dl></>:<EmptyState>Enter a run ID to inspect an engineering run.</EmptyState>}
      </Panel>
      <Panel title="Agents / Tasks" className="tasks">
        {tasks.length?tasks.map(t=><article className="task" key={t.id}><div><strong>{t.role}</strong><span>{t.title}</span></div><StatusPill status={t.status}/></article>):<EmptyState>No task graph loaded.</EmptyState>}
      </Panel>
      <Panel title="Task Graph" className="graph">
        {tasks.length?<svg viewBox={`0 0 800 ${Math.max(220,tasks.length*62)}`} role="img" aria-label="Task dependency graph">
          {tasks.flatMap((t,i)=>t.dependencies.map(dep=>{const j=tasks.findIndex(x=>x.id===dep);if(j<0)return null;return <line key={dep+t.id} x1="210" y1={j*58+34} x2="570" y2={i*58+34} stroke="currentColor" opacity=".25"/>}))}
          {tasks.map((t,i)=><g key={t.id} transform={`translate(${i%2?530:20} ${i*58+10})`}><rect width="250" height="44" rx="10"/><text x="12" y="18">{t.role}</text><text className="sub" x="12" y="34">{t.title.slice(0,32)}</text></g>)}
        </svg>:<EmptyState>The DAG appears when tasks are planned.</EmptyState>}
      </Panel>
      <Panel title="Audit / History" className="events">
        {events.length?<ol>{events.slice().reverse().map(e=><li key={e.event_id}><time>{new Date(e.timestamp).toLocaleTimeString()}</time><strong>{e.actor.id}</strong><span>{e.event_type}</span></li>)}</ol>:<EmptyState>No events loaded.</EmptyState>}
      </Panel>
      <Panel title="Security & Cost" className="metrics">
        <div className="metric"><span>Spent</span><strong>${cost.toFixed(4)}</strong></div>
        <div className="metric"><span>Tasks</span><strong>{tasks.length}</strong></div>
        <div className="metric"><span>Completed</span><strong>{tasks.filter(t=>t.status==="completed").length}</strong></div>
        <div className="metric"><span>Failed</span><strong>{tasks.filter(t=>t.status==="failed").length}</strong></div>
      </Panel>
    </div>
  </main>
}
