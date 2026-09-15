import { useCallback, useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { pageUrl, seepdfOrigin, seepdfUrl } from "./spike/seepdfUrl";
import { BENCH_BYTES, runIpcBench, type BenchRow } from "./spike/bench";
import "./spike/spike.css";

// ---- types mirrored from the Rust spike ----
interface DocInfo { id: string; path: string; pageCount: number; openMs: number }
interface SpikeInfo { pdfiumDir: string; resourceDir: string | null; argv: string[]; os: string; arch: string; debug: boolean }
type JobEvent =
  | { type: "started"; jobId: number; total: number }
  | { type: "progress"; jobId: number; done: number; total: number }
  | { type: "done"; jobId: number; elapsedMs: number }
  | { type: "cancelled"; jobId: number; done: number };
type JobMode = "channel" | "event" | "both";
interface JobView { id: number; done: number; total: number; status: string }

const PDF_FILTER = [{ name: "PDF", extensions: ["pdf"] }];

/** Mirror interesting results into the Rust log (tracing target "webview"). */
function log(msg: string) {
  console.log(msg);
  void invoke("spike_log", { level: "info", msg });
}

function App() {
  const [ping, setPing] = useState("…");
  const [info, setInfo] = useState<SpikeInfo | null>(null);
  const [docs, setDocs] = useState<DocInfo[]>([]);
  const [docId, setDocId] = useState("tracemonkey");
  const [pageNo, setPageNo] = useState(0);
  const [scale, setScale] = useState(1.5);
  const [protoMs, setProtoMs] = useState<number | null>(null);
  const [probes, setProbes] = useState<string[]>([]);
  const [ipcImg, setIpcImg] = useState<{ url: string; ms: number; bytes: number } | null>(null);
  const [bench, setBench] = useState<BenchRow[]>([]);
  const [benchState, setBenchState] = useState<"idle" | "running" | "done">("idle");
  const [job, setJob] = useState<JobView | null>(null);
  const [jobResults, setJobResults] = useState<string[]>([]);
  const [over, setOver] = useState(false);
  const [events, setEvents] = useState<string[]>([]);
  const [tick, setTick] = useState(0);
  const [slow, setSlow] = useState("");
  const [fsRead, setFsRead] = useState("");
  const [savePath, setSavePath] = useState("");
  const [cspMeta, setCspMeta] = useState("probing…");
  const imgStart = useRef(0);
  const tickRef = useRef(0);
  const started = useRef(false);
  const currentJob = useRef<number | null>(null);

  const addEvent = useCallback((s: string) => {
    setEvents((e) => [...e.slice(-12), `${new Date().toLocaleTimeString()}  ${s}`]);
  }, []);

  const refreshDocs = useCallback(async () => {
    const d = await invoke<DocInfo[]>("list_documents");
    d.sort((a, b) => a.id.localeCompare(b.id));
    setDocs(d);
    return d;
  }, []);

  const openPath = useCallback(async (path: string, source: string) => {
    try {
      const d = await invoke<DocInfo>("open_document", { path });
      addEvent(`${source}: opened "${d.id}" (${d.pageCount} pages, ${d.openMs.toFixed(1)} ms) ${path}`);
      log(`${source}: opened ${d.id} ${d.pageCount} pages ${path}`);
      await refreshDocs();
      setDocId(d.id);
      setPageNo(0);
    } catch (e) {
      addEvent(`${source}: open failed: ${String(e)}`);
    }
  }, [addEvent, refreshDocs]);

  // ---- progress job (Channel and/or event bus), resolves on done/cancel ----
  const runJob = useCallback((mode: JobMode, steps: number, stepMs: number) => {
    return new Promise<string>((resolve) => {
      const t0 = performance.now();
      let count = 0;
      let unlisten: (() => void) | null = null;
      const finish = (summary: string) => {
        unlisten?.();
        currentJob.current = null;
        setJobResults((r) => [...r, summary]);
        log(summary);
        resolve(summary);
      };
      const handle = (ev: JobEvent, via: string) => {
        if (ev.type === "started") {
          currentJob.current = ev.jobId;
          setJob({ id: ev.jobId, done: 0, total: ev.total, status: `running via ${via}` });
        } else if (ev.type === "progress") {
          count++;
          if (ev.done % Math.max(1, Math.floor(ev.total / 20)) === 0 || ev.done === ev.total) {
            setJob((j) => (j ? { ...j, done: ev.done } : j));
          }
        } else if (ev.type === "done") {
          const ms = performance.now() - t0;
          setJob((j) => (j ? { ...j, done: j.total, status: `done via ${via}` } : j));
          finish(`${mode}: ${steps} steps, ${count} msgs received in ${ms.toFixed(0)} ms (${((count * 1000) / ms).toFixed(0)} msg/s), rust side ${ev.elapsedMs.toFixed(0)} ms`);
        } else if (ev.type === "cancelled") {
          setJob((j) => (j ? { ...j, done: ev.done, status: `cancelled via ${via}` } : j));
          finish(`${mode}: cancelled at ${ev.done}/${steps} after ${(performance.now() - t0).toFixed(0)} ms`);
        }
      };
      const ch = new Channel<JobEvent>();
      if (mode !== "event") ch.onmessage = (ev) => handle(ev, "Channel");
      const start = () => invoke<number>("start_job", { steps, stepMs, mode, onProgress: ch });
      if (mode !== "channel") {
        void listen<JobEvent>("job-progress", (e) => handle(e.payload, "event")).then((u) => { unlisten = u; return start(); });
      } else {
        void start();
      }
    });
  }, []);

  const cancelJob = useCallback(async () => {
    if (currentJob.current == null) return;
    const ok = await invoke<boolean>("cancel_job", { jobId: currentJob.current });
    addEvent(`cancel_job(${currentJob.current}) -> ${ok}`);
  }, [addEvent]);

  // ---- seepdf:// probes: headers, 304, Range/206, 404, 500 ----
  const probeProtocol = useCallback(async (doc: string) => {
    const out: string[] = [];
    const probe = async (label: string, f: () => Promise<string>) => {
      try { out.push(`${label} -> ${await f()}`); } catch (e) { out.push(`${label} -> THREW ${String(e)}`); }
      setProbes([...out]);
    };
    const url = pageUrl(doc, 0, 1.5);
    let etag = "";
    await probe("GET /page", async () => {
      const r = await fetch(url);
      etag = r.headers.get("etag") ?? "";
      return `${r.status} ${r.headers.get("content-type")}, ${r.headers.get("content-length")} B, render ${r.headers.get("x-render-ms")} ms, encode ${r.headers.get("x-encode-ms")} ms, ${r.headers.get("x-image-width")}x${r.headers.get("x-image-height")}, etag ${etag}`;
    });
    await probe("GET /page again (does WebKit revalidate with If-None-Match? see rust log)", async () => `${(await fetch(url)).status}`);
    await probe("GET /page + manual If-None-Match (expect 304)", async () => `${(await fetch(url, { headers: { "If-None-Match": etag } })).status}`);
    await probe("GET /raw Range 0-1023 (expect 206)", async () => { const r = await fetch(seepdfUrl("/raw", { doc }), { headers: { Range: "bytes=0-1023" } }); return `${r.status} ${r.headers.get("content-range")} ${(await r.arrayBuffer()).byteLength} B`; });
    await probe("GET /nope (expect 404)", async () => `${(await fetch(seepdfUrl("/nope"))).status}`);
    await probe("GET /page doc=missing (expect 404)", async () => `${(await fetch(pageUrl("missing", 0, 1))).status}`);
    await probe("GET /page page=9999 (expect 500)", async () => { const r = await fetch(pageUrl(doc, 9999, 1)); return `${r.status} "${(await r.text()).slice(0, 40)}"`; });
    await probe("GET /page tile out of range (expect 500)", async () => `${(await fetch(pageUrl(doc, 0, 1, { tx: 99, ty: 0, tile: 256 }))).status}`);
    out.forEach((l) => log(`probe ${l}`));
  }, []);

  const renderViaIpc = useCallback(async (doc: string, page: number, sc: number) => {
    const t0 = performance.now();
    const buf = await invoke<ArrayBuffer>("render_page_png_ipc", { doc, page, scale: sc });
    const url = URL.createObjectURL(new Blob([buf], { type: "image/png" }));
    const ms = performance.now() - t0;
    setIpcImg((old) => { if (old) URL.revokeObjectURL(old.url); return { url, ms, bytes: buf.byteLength }; });
    log(`page via IPC (render_page_png_ipc): ${buf.byteLength} B in ${ms.toFixed(1)} ms`);
  }, []);

  const fetchSlow = useCallback(async () => {
    const t0 = performance.now();
    const tick0 = tickRef.current;
    setSlow("waiting for seepdf://…/slow?ms=2000 (ticker must keep running)…");
    const r = await fetch(seepdfUrl("/slow", { ms: 2000 }));
    const ticks = tickRef.current - tick0;
    const msg = `${await r.text()}; UI ticker advanced ${ticks} times in ${(performance.now() - t0).toFixed(0)} ms => main thread not blocked`;
    setSlow(msg);
    log(msg);
  }, []);

  const openDialog = useCallback(async () => {
    const p = await invoke<string | null>("plugin:dialog|open", {
      options: { title: "Open PDF", multiple: false, directory: false, filters: PDF_FILTER },
    });
    if (p) await openPath(p, "dialog"); else addEvent("dialog: cancelled");
  }, [openPath, addEvent]);

  const saveDialog = useCallback(async () => {
    const p = await invoke<string | null>("plugin:dialog|save", {
      options: { title: "Save PDF as", defaultPath: "copy.pdf", filters: PDF_FILTER },
    });
    setSavePath(p ?? "(cancelled)");
  }, []);

  const readViaFs = useCallback(async (path: string) => {
    try {
      const t0 = performance.now();
      const buf = await invoke<ArrayBuffer>("plugin:fs|read_file", { path, options: {} });
      setFsRead(`plugin:fs|read_file -> ${buf.byteLength} bytes in ${(performance.now() - t0).toFixed(1)} ms (${path})`);
    } catch (e) {
      setFsRead(`plugin:fs|read_file failed: ${String(e)}`);
    }
  }, []);

  // ---- boot sequence (runs once; StrictMode double-invoke guarded) ----
  useEffect(() => {
    if (started.current) return;
    started.current = true;
    (async () => {
      setPing(await invoke<string>("engine_ping"));
      // CSP probe: tauri v2 delivers the CSP as an HTTP header on tauri://localhost (no <meta>), so the only
      // observable is a securitypolicyviolation event when something disallowed is attempted.
      await new Promise<void>((resolve) => {
        const hits = new Set<string>();
        const onViolation = (e: SecurityPolicyViolationEvent) => hits.add(e.violatedDirective || e.effectiveDirective);
        document.addEventListener("securitypolicyviolation", onViolation);
        const img = new Image();
        img.src = "http://csp-probe.invalid/x.png";
        fetch("http://csp-probe.invalid/").catch(() => undefined);
        setTimeout(() => {
          document.removeEventListener("securitypolicyviolation", onViolation);
          const msg = hits.size ? `CSP active — violations reported for: ${[...hits].join(", ")}` : "CSP not applied (no securitypolicyviolation events; dev server HTML)";
          setCspMeta(msg);
          log(msg);
          resolve();
        }, 1500);
      });
      const i = await invoke<SpikeInfo>("spike_info");
      setInfo(i);
      const d = await refreshDocs();
      for (const p of await invoke<string[]>("take_pending_opens")) await openPath(p, "pending");
      const first = d.find((x) => x.id === "tracemonkey") ?? d[0];
      const step = async (name: string, f: () => Promise<unknown>) => {
        try { await f(); } catch (e) { addEvent(`${name} failed: ${String(e)}`); log(`${name} failed: ${String(e)}`); }
      };
      if (first) {
        await step("probes", () => probeProtocol(first.id));
        await step("ipc render", () => renderViaIpc(first.id, 0, 1.5));
        await step("fs read", () => readViaFs(first.path));
      }
      setBenchState("running");
      await step("bench", () => runIpcBench(BENCH_BYTES, (row) => {
        setBench((r) => [...r, row]);
        log(`IPC ${row.name}: ${row.ms.toFixed(1)} ms ${row.ok ? "OK" : "FAIL"} ${row.note ?? ""}`);
      }));
      setBenchState("done");
      for (let i = 0; i < 3; i++) {
        await step("job channel", () => runJob("channel", 2000, 0));
        await step("job event", () => runJob("event", 2000, 0));
      }
      await step("job cancel", () => { const p = runJob("channel", 400, 50); setTimeout(() => void cancelJob(), 1500); return p; });
      await step("slow", fetchSlow);
    })().catch((e) => addEvent(`boot failed: ${String(e)}`));
  }, [addEvent, cancelJob, fetchSlow, openPath, probeProtocol, readViaFs, refreshDocs, renderViaIpc, runJob]);

  // ---- listeners: open-file (argv / Finder), drag-drop ----
  useEffect(() => {
    const un: Array<() => void> = [];
    let alive = true;
    void listen<{ path: string; source: string }>("open-file", (e) => void openPath(e.payload.path, e.payload.source))
      .then((u) => (alive ? un.push(u) : u()));
    void getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "over") return;
      if (e.payload.type === "enter") setOver(true);
      if (e.payload.type === "leave") setOver(false);
      if (e.payload.type === "drop") {
        setOver(false);
        addEvent(`drop: ${e.payload.paths.join(", ")} @ (${e.payload.position.x}, ${e.payload.position.y})`);
        for (const p of e.payload.paths) if (p.toLowerCase().endsWith(".pdf")) void openPath(p, "drag-drop");
      }
    }).then((u) => (alive ? un.push(u) : u()));
    return () => { alive = false; un.forEach((u) => u()); };
  }, [openPath, addEvent]);

  // ---- UI ticker (proves the webview keeps painting while the engine sleeps) ----
  useEffect(() => {
    const id = setInterval(() => { tickRef.current += 1; setTick(tickRef.current); }, 100);
    return () => clearInterval(id);
  }, []);

  const doc = docs.find((d) => d.id === docId);
  const url = doc ? pageUrl(doc.id, pageNo, scale) : "";
  useEffect(() => { imgStart.current = performance.now(); setProtoMs(null); }, [url]);

  return (
    <>
      <div className="titlebar" data-tauri-drag-region>
        SeePDF <span className="muted">Tauri v2 integration spike</span>
        <span className="muted mono" style={{ marginLeft: "auto" }}>{info ? `${info.os}/${info.arch} ${info.debug ? "debug" : "release"}` : ""}</span>
      </div>
      <div className="grid">
        <div className="panel">
          <h2>Engine (managed state, crossbeam → engine thread)</h2>
          <div className="mono">engine_ping: {ping}</div>
          <div className="mono muted">pdfium dir: {info?.pdfiumDir}</div>
          <div className="mono muted">resource_dir(): {info?.resourceDir ?? "n/a"}</div>
          <div className="mono muted">argv: {info?.argv.join(" ")}</div>
          <div className="mono muted">CSP: {cspMeta}</div>
          <div className="row">
            <span>documents:</span>
            {docs.map((d) => (
              <button key={d.id} onClick={() => { setDocId(d.id); setPageNo(0); }} style={{ fontWeight: d.id === docId ? 700 : 400 }}>
                {d.id} ({d.pageCount}p)
              </button>
            ))}
          </div>
        </div>

        <div className="panel">
          <h2>File open: dialog / drag-drop / argv / Finder</h2>
          <div className="row">
            <button onClick={openDialog}>Open… (plugin:dialog|open)</button>
            <button onClick={saveDialog}>Save as… (plugin:dialog|save)</button>
            <span className="mono muted">{savePath}</span>
          </div>
          <div className={`drop ${over ? "over" : ""}`}>drop a .pdf anywhere on the window (getCurrentWebview().onDragDropEvent)</div>
          <div className="mono muted">{fsRead}</div>
          <div className="log mono">{events.map((e, i) => <div key={i}>{e}</div>)}</div>
        </div>

        <div className="panel">
          <h2>seepdf:// protocol — origin <code>{seepdfOrigin()}</code></h2>
          <div className="row">
            <button onClick={() => setPageNo((p) => Math.max(0, p - 1))}>◀</button>
            <span>page {pageNo + 1}/{doc?.pageCount ?? 0}</span>
            <button onClick={() => setPageNo((p) => Math.min((doc?.pageCount ?? 1) - 1, p + 1))}>▶</button>
            <label>scale <input type="number" step="0.25" min="0.25" max="6" value={scale} onChange={(e) => setScale(Number(e.target.value))} style={{ width: 60 }} /></label>
            <button onClick={() => doc && renderViaIpc(doc.id, pageNo, scale)}>render same page via binary IPC</button>
          </div>
          <div className="page">
            <div>
              {url && (
                <img
                  src={url}
                  alt="page via seepdf://"
                  onLoad={() => setProtoMs(performance.now() - imgStart.current)}
                  onError={() => setProtoMs(-1)}
                />
              )}
              <div className="mono">
                &lt;img src="seepdf://…"&gt;: {protoMs == null ? "loading…" : protoMs < 0 ? <span className="bad">ERROR</span> : <span className="ok">{protoMs.toFixed(1)} ms</span>}
              </div>
            </div>
            <div>
              {doc && (
                <div className="tiles">
                  {[[0, 0], [1, 0], [0, 1], [1, 1]].map(([tx, ty]) => (
                    <img key={`${tx}-${ty}`} src={pageUrl(doc.id, pageNo, scale, { tx, ty, tile: 256 })} alt={`tile ${tx},${ty}`} />
                  ))}
                </div>
              )}
              <div className="mono muted">4 tiles: ?tx=&amp;ty=&amp;tile=256</div>
            </div>
            <div>
              {ipcImg && <img src={ipcImg.url} alt="page via IPC" />}
              <div className="mono">
                via IPC (ArrayBuffer→Blob URL): {ipcImg ? <span className="ok">{ipcImg.ms.toFixed(1)} ms, {(ipcImg.bytes / 1024).toFixed(0)} KB</span> : "…"}
              </div>
            </div>
          </div>
          <div className="mono muted" style={{ wordBreak: "break-all" }}>{url}</div>
          <ul className="mono" style={{ margin: "6px 0 0", paddingLeft: 18 }}>
            {probes.map((p, i) => <li key={i}>{p}</li>)}
          </ul>
        </div>

        <div className="panel">
          <h2>Binary IPC benchmark — {(BENCH_BYTES / 1048576).toFixed(0)} MB payload, {benchState}</h2>
          <table>
            <thead><tr><th>transport</th><th>ms</th><th>ok</th><th>note</th></tr></thead>
            <tbody>
              {bench.map((r) => (
                <tr key={r.name}>
                  <td>{r.name}</td>
                  <td className="num">{r.ms.toFixed(1)}</td>
                  <td className={r.ok ? "ok" : "bad"}>{r.ok ? "OK" : "FAIL"}</td>
                  <td className="muted">{r.note}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>

        <div className="panel">
          <h2>Cancellable job — tauri::ipc::Channel vs app.emit</h2>
          <div className="row">
            <button onClick={() => runJob("channel", 2000, 0)}>2000 msgs via Channel</button>
            <button onClick={() => runJob("event", 2000, 0)}>2000 msgs via emit</button>
            <button onClick={() => runJob("channel", 400, 50)}>20 s job</button>
            <button onClick={cancelJob}>Cancel</button>
          </div>
          {job && (
            <>
              <progress value={job.done} max={job.total} />
              <div className="mono">job #{job.id}: {job.done}/{job.total} — {job.status}</div>
            </>
          )}
          <ul className="mono" style={{ margin: "6px 0 0", paddingLeft: 18 }}>
            {jobResults.map((r, i) => <li key={i}>{r}</li>)}
          </ul>
        </div>

        <div className="panel">
          <h2>Main thread stays free</h2>
          <div className="row">
            <span className="ticker mono">ticker: {tick}</span>
            <button onClick={fetchSlow}>fetch seepdf://…/slow?ms=2000</button>
            <button onClick={async () => { const t0 = performance.now(); await invoke("engine_sleep", { ms: 1500 }); addEvent(`engine_sleep(1500) awaited in ${(performance.now() - t0).toFixed(0)} ms`); }}>await engine_sleep(1500)</button>
          </div>
          <div className="mono">{slow}</div>
        </div>
      </div>
    </>
  );
}

export default App;
