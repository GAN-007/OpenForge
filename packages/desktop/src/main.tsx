import React from "react";
import { createRoot } from "react-dom/client";
import "@xterm/xterm/css/xterm.css";
import { DesktopApp } from "./DesktopApp";
import "./styles.css";

const root = document.getElementById("root");
if (!root) throw new Error("OpenForge root element is missing");

createRoot(root).render(
  <React.StrictMode>
    <DesktopApp />
  </React.StrictMode>,
);
