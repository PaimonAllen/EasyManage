import { lazy, StrictMode, Suspense } from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

// History is a standalone entry: mounting it never starts App/useFleet or the
// live metrics, SSH-target and dashboard history polling hooks.
const Page=window.location.pathname.replace(/\/$/,"")==="/usage-history"
  ?lazy(()=>import("./pages/UsageHistory"))
  :lazy(()=>import("./app/App").then(module=>({default:module.App})));

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <Suspense fallback={<div className="loading">正在载入页面…</div>}><Page/></Suspense>
  </StrictMode>,
);
