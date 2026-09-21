import type {PropsWithChildren,ReactNode} from "react";

export function Panel({title,actions,children,className=""}:PropsWithChildren<{title:string;actions?:ReactNode;className?:string}>){
  return <section className={`of-panel ${className}`}>
    <header className="of-panel__header"><strong>{title}</strong><span>{actions}</span></header>
    <div className="of-panel__body">{children}</div>
  </section>;
}
export function StatusPill({status}:{status:string}){
  return <span className={`of-status of-status--${status.replaceAll("_","-")}`}>{status.replaceAll("_"," ")}</span>;
}
export function EmptyState({children}:PropsWithChildren){return <div className="of-empty">{children}</div>;}

export { GatewaySettings } from "./GatewaySettings.js";
