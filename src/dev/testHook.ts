/**
 * The browser half of the dev-only command bridge (`scripts/dev-bridge.mjs`).
 *
 * Loaded by `App.tsx` only under `import.meta.env.DEV`, and the endpoints it polls only exist
 * in the Vite dev server, so this cannot reach a shipped bundle. It is how the Stage 2
 * end-to-end smoke drives the **real** app (real engine, real pdfium) from a shell instead of
 * from a devtools console: every step is a command id, an IPC call or a state read-back, which
 * makes the run reproducible and the evidence checkable.
 *
 * Nothing here is `eval`: the CSP of the shipped app forbids it, and a fixed op table is enough.
 */
import * as api from "../ipc/api";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { paneView, useViewStore } from "../store/viewStore";
import { useAnnotStore } from "../store/annotStore";
import { usePagesStore } from "../store/pagesStore";
import { useJobStore } from "../store/jobStore";
import { useToastStore } from "../app/toastStore";
import { useDialogStore } from "../dialogs/dialogState";
import type { JobEvent } from "../ipc/types";
import { t, type TParams } from "../i18n";

type Run = (id: string) => void;

interface Cmd {
  op: string;
  [k: string]: unknown;
}

function snapshot(): unknown {
  const app = useAppStore.getState();
  const doc = useDocStore.getState();
  const view = useViewStore.getState();
  const annots = useAnnotStore.getState();
  const pages = usePagesStore.getState();
  return {
    ready: app.ready,
    mode: app.mode,
    tool: app.tool,
    locale: app.locale,
    theme: app.theme,
    sidebarTab: app.sidebarTab,
    recents: app.recents.length,
    status: doc.status,
    error: doc.error,
    doc: doc.info && {
      docId: doc.info.docId,
      name: doc.info.name,
      path: doc.info.path,
      pageCount: doc.info.pageCount,
      generation: doc.info.docGeneration,
      dirty: doc.info.dirty,
      canUndo: doc.info.canUndo,
      canRedo: doc.info.canRedo,
      hasForm: doc.info.hasForm,
      encrypted: doc.info.encrypted,
      firstPage: doc.info.pages[0],
    },
    outline: doc.outline.length,
    view: { page: view.currentPage, zoom: view.zoomPercent, layout: view.layout, rotation: view.rotation },
    annots: {
      total: annots.all().length,
      byPage: Object.fromEntries(Object.entries(annots.byPage).map(([p, list]) => [p, list.length])),
      selected: annots.selected,
      ghosts: annots.ghosts.length,
    },
    pages: { selected: pages.selected, focus: pages.focus },
    jobs: useJobStore.getState().jobs.map((j) => ({ id: j.id, kind: j.kind, done: j.done, total: j.total })),
    toasts: useToastStore.getState().toasts.map((t) => ({ tone: t.tone, key: t.messageKey, detail: t.detail })),
    dialogs: useDialogStore.getState().stack.map((d) => d.name),
  };
}

/** Toasts newer than `after` (a toast id), with their text rendered in the active locale. */
function renderedToasts(after: number): unknown[] {
  return useToastStore
    .getState()
    .toasts.filter((x) => x.id > after)
    .map((x) => ({ tone: x.tone, key: x.messageKey, params: x.params, text: t(x.messageKey, x.params), actions: x.actions?.map((a) => t(a.labelKey)) }));
}

/** Commands that stream progress take a channel; collect it and resolve on `done`. */
function withJob<T>(fn: (onProgress: (e: JobEvent) => void) => Promise<T>): Promise<unknown> {
  const events: JobEvent[] = [];
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (value: unknown) => {
      if (settled) return;
      settled = true;
      resolve(value);
    };
    void fn((e) => {
      events.push(e);
      if (e.type === "done") finish({ events, outputs: e.outputs });
      if (e.type === "error") finish({ events, error: e.error });
      if (e.type === "cancelled") finish({ events, cancelled: true });
    })
      .then((value) => setTimeout(() => finish({ events, value }), 400))
      .catch(reject);
  });
}

/** `docId` defaults to the open document, so a smoke step does not have to thread it. */
function withDocId(args: unknown): Record<string, unknown> {
  const out = { ...((args as Record<string, unknown>) ?? {}) };
  if (out.docId === undefined) {
    const id = useDocStore.getState().docId;
    if (id) out.docId = id;
  }
  return out;
}

async function execute(cmd: Cmd, run: Run): Promise<unknown> {
  switch (cmd.op) {
    case "ping":
      return { pong: true, dev: true };
    case "state":
      return snapshot();
    case "run":
      run(String(cmd.id));
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 250)));
      return snapshot();
    case "open": {
      const m = await import("../dialogs/flows");
      await m.openPaths([String(cmd.path)]);
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 400)));
      return snapshot();
    }
    /**
     * F-30 `open.*` rows: open a file and wait for the **first painted pixel**.
     *
     * `PageShell.markLoaded` stamps `window.__seepdfFirstPaint` on the first `<img>` that
     * lands (placeholder, whole-page bitmap or tile), and `Viewer` stamps
     * `window.__seepdfOpenAt` when the document reaches the UI. Both are cleared here so the
     * number is this open's, not the session's, and the clock starts before `openPaths` so
     * `open_document` itself is inside the measurement — that is what the budget means.
     */
    case "openTimed": {
      const m = await import("../dialogs/flows");
      window.__seepdfFirstPaint = undefined;
      window.__seepdfOpenAt = performance.now();
      const started = performance.now();
      await m.openPaths([String(cmd.path)]);
      const documentMs = performance.now() - started;
      const deadline = performance.now() + Number(cmd.timeoutMs ?? 20_000);
      while (window.__seepdfFirstPaint === undefined && performance.now() < deadline) {
        await new Promise((r) => setTimeout(r, 8));
      }
      const paint = window.__seepdfFirstPaint;
      return {
        documentMs,
        firstPaintMs: paint === undefined ? null : paint - started,
        state: snapshot(),
      };
    }
    /** F-30 `scroll.*` / `rss.*` rows: walk the document, then read the engine's own stats. */
    case "scrollThrough": {
      const view = useViewStore.getState();
      const info = useDocStore.getState().info;
      if (!info) throw new Error("no document");
      const step = Number(cmd.step ?? 25);
      const settleMs = Number(cmd.settleMs ?? 120);
      const started = performance.now();
      let stops = 0;
      for (let page = 0; page < info.pageCount; page += step) {
        view.goToPage(page);
        stops++;
        await new Promise((r) => setTimeout(r, settleMs));
      }
      return { stops, elapsedMs: performance.now() - started, state: snapshot() };
    }
    case "openWithPassword": {
      const info = await useDocStore.getState().open(String(cmd.path), String(cmd.password));
      return { info: info && { docId: info.docId, pageCount: info.pageCount }, state: snapshot() };
    }
    case "store": {
      const stores: Record<string, { getState(): object }> = {
        app: useAppStore,
        doc: useDocStore,
        view: useViewStore,
        annot: useAnnotStore,
        pages: usePagesStore,
      };
      const store = stores[String(cmd.store)];
      if (!store) throw new Error(`unknown store ${String(cmd.store)}`);
      const fn = (store.getState() as Record<string, unknown>)[String(cmd.method)];
      if (typeof fn !== "function") throw new Error(`unknown method ${String(cmd.method)}`);
      const out = await (fn as (...a: unknown[]) => unknown)(...((cmd.args as unknown[]) ?? []));
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 200)));
      return { out: out ?? null, state: snapshot() };
    }
    case "api": {
      const table = api as unknown as Record<string, (...a: unknown[]) => unknown>;
      const fn = table[String(cmd.fn)];
      if (typeof fn !== "function") throw new Error(`unknown api ${String(cmd.fn)}`);
      return await fn(withDocId(cmd.args));
    }
    case "annotate": {
      const m = await import("../annot/actions");
      const created = await m.createAnnotation(Number(cmd.page), cmd.spec as never);
      await new Promise((r) => setTimeout(r, 350));
      return { created, state: snapshot() };
    }
    case "flushAnnots": {
      const m = await import("../annot/actions");
      await m.flushPatches();
      await new Promise((r) => setTimeout(r, 250));
      return snapshot();
    }
    /**
     * Call one exported function of `dialogs/flows` — the seam the smoke needs for the flows
     * that are only reachable from a dialog button (`runPrint`, `confirmUnsaved`, …). A fixed
     * module, so this is still a table and not an `eval`.
     */
    case "flow": {
      const m = (await import("../dialogs/flows")) as unknown as Record<string, unknown>;
      const fn = m[String(cmd.fn)];
      if (typeof fn !== "function") throw new Error(`unknown flow ${String(cmd.fn)}`);
      const out = await (fn as (...a: unknown[]) => unknown)(...((cmd.args as unknown[]) ?? []));
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 300)));
      return { out: out ?? null, state: snapshot() };
    }
    case "pageOps": {
      const m = await import("../dialogs/flows");
      await m.runPageOps(cmd.ops as never);
      await new Promise((r) => setTimeout(r, 400));
      return snapshot();
    }
    case "save": {
      const info = useDocStore.getState().info;
      if (!info) throw new Error("no document");
      const path = cmd.path ? String(cmd.path) : null;
      const out = await withJob((onProgress) =>
        path
          ? api.saveDocumentAs({ docId: info.docId, path }, onProgress)
          : api.saveDocument({ docId: info.docId }, onProgress),
      );
      // `flows.saveAsFlow` refreshes the store after a Save As (the document's path and its
      // dirty flag move); this op calls the API directly, so it has to do the same or the
      // snapshot reports a dirty document that is in fact saved.
      await useDocStore.getState().refresh();
      await new Promise((r) => setTimeout(r, 250));
      return { out, state: snapshot() };
    }
    case "job": {
      const table = api as unknown as Record<string, (...a: unknown[]) => unknown>;
      const fn = table[String(cmd.fn)];
      if (typeof fn !== "function") throw new Error(`unknown api ${String(cmd.fn)}`);
      return await withJob((onProgress) => fn(withDocId(cmd.args), onProgress) as Promise<unknown>);
    }
    case "ocr": {
      const m = await import("../ocr");
      const info = useDocStore.getState().info;
      if (!info) throw new Error("no document");
      const result = await m.runOcrJob({
        docId: info.docId,
        docGeneration: info.docGeneration,
        pages: (cmd.pages as number[]) ?? [0],
        pageGeom: info.pages,
        langs: String(cmd.langs ?? "kor+eng"),
        dpi: (cmd.dpi as 300) ?? 300,
        skipPagesWithText: Boolean(cmd.skipPagesWithText ?? false),
        replaceExisting: Boolean(cmd.replaceExisting ?? true),
        engine: cmd.engine === undefined ? undefined : (String(cmd.engine) as "tesseract" | "vision"),
      });
      await new Promise((r) => setTimeout(r, 400));
      return { result, state: snapshot() };
    }
    case "search": {
      const info = useDocStore.getState().info;
      if (!info) throw new Error("no document");
      const hits: unknown[] = [];
      await new Promise<void>((resolve, reject) => {
        void api
          .searchStart(
            { docId: info.docId, query: String(cmd.query), matchCase: false, wholeWord: false, fromPage: 0 },
            (e) => {
              if (e.type === "page") hits.push(...e.hits.map((h) => ({ page: h.page, context: h.context })));
              if (e.type === "done" || e.type === "cancelled" || e.type === "error") resolve();
            },
          )
          .catch(reject);
      });
      return { count: hits.length, hits: hits.slice(0, 8) };
    }
    case "text": {
      const info = useDocStore.getState().info;
      if (!info) throw new Error("no document");
      const text = await api.getPageText({ docId: info.docId, page: Number(cmd.page ?? 0) });
      return { length: text.length, head: text.slice(0, Number(cmd.chars ?? 300)) };
    }
    case "selection": {
      const m = await import("../viewer/viewerCommands");
      const dom = typeof document === "undefined" ? null : document.getSelection();
      const mirror = document.getElementById("seepdf-clipboard-mirror");
      return {
        engineText: m.currentSelectionText().slice(0, 200),
        engineChars: m.currentSelectionText().length,
        domSelection: dom?.toString().slice(0, 200) ?? null,
        domChars: dom?.toString().length ?? 0,
        mirrorChars: mirror?.textContent?.length ?? -1,
        mirrorInSelection: Boolean(dom?.anchorNode && mirror?.contains(dom.anchorNode)),
        activeElement: document.activeElement?.className ?? document.activeElement?.tagName ?? null,
      };
    }
    case "copy": {
      const m = await import("../viewer/viewerCommands");
      const text = m.currentSelectionText();
      return { ok: m.copyToClipboard(text), chars: text.length };
    }
    /**
     * Stage 9 real-app QA: the 텍스트 수정 path end to end, as a click with the tool and a 완료 would
     * run it. `beginParagraphEdit(page, at)` opens the editor on the paragraph under `at`, then
     * `commitSession(text)` runs the push dry run → commit, or the blocked prompt → the chosen flow →
     * toast. `text` replaces the paragraph; without it the probe's text + `append` is written.
     * Prompts that open meanwhile are answered from `answers` (`confirm`: the Hangul font consent,
     * default false; `choice`: fit / overlap / keepEditing, default the prompt's cancel) and reported
     * with their rendered text; the toasts raised by the commit come back rendered too. An editor
     * left open (계속 편집, a declined font) is closed afterwards unless `keepOpen`.
     */
    case "paragraphUi": {
      const m = await import("../edit/actions");
      const { useEditStore } = await import("../edit/editStore");
      const info = useDocStore.getState().info;
      if (!info) throw new Error("no document");
      const page = Number(cmd.page ?? 0);
      await m.loadPage(info.docId, page, info.docGeneration);
      const began = await m.beginParagraphEdit(page, cmd.at as [number, number]);
      const session = useEditStore.getState().session;
      if (!began || session?.kind !== "paragraph") return { began, toasts: renderedToasts(0), state: snapshot() };
      const probe = session.probe;
      const text = cmd.text !== undefined ? String(cmd.text) : probe.text + String(cmd.append ?? "");
      const answers = (cmd.answers ?? {}) as { confirm?: boolean; choice?: string };
      const lastToast = Math.max(0, ...useToastStore.getState().toasts.map((x) => x.id));
      const prompts: unknown[] = [];
      let settled = false;
      const commit = m.commitSession(text).finally(() => {
        settled = true;
      });
      while (!settled) {
        const top = useDialogStore.getState().stack.at(-1);
        if (top && (top.name === "confirm" || top.name === "choice")) {
          const { resolve, ...req } = top.props as { resolve: (v: unknown) => void } & Record<string, unknown>;
          const r = req as {
            titleKey: string; bodyKey: string; bodyParams?: TParams; hintKey?: string; hintParams?: TParams;
            options?: { value: string; labelKey: string; labelParams?: TParams; disabled?: boolean }[];
            cancel?: { value: string; labelKey: string };
          };
          prompts.push({
            name: top.name,
            ...req,
            text: {
              title: t(r.titleKey),
              body: t(r.bodyKey, r.bodyParams),
              hint: r.hintKey ? t(r.hintKey, r.hintParams) : undefined,
              options: r.options?.map((o) => `${t(o.labelKey, o.labelParams)}${o.disabled ? " (disabled)" : ""}`),
            },
          });
          if (top.name === "confirm") resolve(Boolean(answers.confirm));
          else resolve(answers.choice ?? r.cancel?.value);
        }
        await new Promise((r) => setTimeout(r, 25));
      }
      const committed = await commit;
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 300)));
      const open = useEditStore.getState().session !== null;
      if (open && !cmd.keepOpen) m.cancelSession();
      return {
        began,
        committed,
        editorLeftOpen: open,
        probe: { rect: probe.rect, lines: probe.lines, strategy: probe.strategy, chars: probe.text.length, fontSizePt: probe.fontSizePt, lineHeightPt: probe.lineHeightPt },
        prompts,
        toasts: renderedToasts(lastToast),
        state: snapshot(),
      };
    }
    /** Webview console errors / warnings and uncaught errors since the bridge started (or `clear`). */
    case "console": {
      const out = consoleLog.slice();
      if (cmd.clear) consoleLog.length = 0;
      return { count: out.length, entries: out.slice(-Number(cmd.limit ?? 50)) };
    }
    /**
     * P2 QA: both panes as the store and the DOM see them — the store's current page per pane, and
     * the page actually at the top of each `.viewer[data-pane]` scroller (what the user sees).
     */
    case "panes":
      return panesSnapshot();
    /**
     * P2 QA: scroll a pane the way a wheel would (its `scrollTop`, no `scrollRequest`), so the page
     * it lands on is not a navigation the scroller could replay.
     */
    case "domScroll": {
      const el = scrollerOf(String(cmd.pane ?? "main"));
      const target = Number(cmd.page);
      // pages are virtualised: step by the height of a laid-out page until the target is in the DOM
      for (let i = 0; i < 40; i++) {
        const shell = el.querySelector<HTMLElement>(`.page-shell[data-page="${target}"]`);
        if (shell) {
          el.scrollTop += shell.getBoundingClientRect().top - el.getBoundingClientRect().top + Number(cmd.offsetPx ?? 20);
          break;
        }
        const any = [...el.querySelectorAll<HTMLElement>(".page-shell[data-page]")];
        if (!any.length) throw new Error("no page laid out");
        const ref = any[0];
        const h = ref.getBoundingClientRect().height + 8;
        el.scrollTop += (target - Number(ref.dataset.page)) * h * 0.9;
        await new Promise((r) => setTimeout(r, 120));
      }
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 500)));
      return panesSnapshot();
    }
    /** P2 QA: type into the status bar's page box and press Enter (its form's submit). */
    case "pageBox": {
      const input = document.querySelector<HTMLInputElement>(".statusbar .page-input");
      if (!input) throw new Error("no page box");
      const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")?.set;
      setter?.call(input, String(cmd.value));
      input.dispatchEvent(new Event("input", { bubbles: true }));
      await new Promise((r) => setTimeout(r, 50));
      input.form?.requestSubmit();
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 600)));
      return { box: input.value, panes: panesSnapshot() };
    }
    /**
     * P2 QA: 여러 파일에서 검색 end to end through `multisearch/flow` — list `paths`, search
     * `query`, answer each 암호 입력 with `password` (or dismiss it like Esc when absent), and report
     * the per-file outcome plus the engine's open-document count before and after.
     */
    case "multiSearch": {
      const m = await import("../multisearch/flow");
      const before = await api.engineStats();
      m.resetMultiSearch();
      m.addFiles((cmd.paths as string[]) ?? []);
      m.setQuery(String(cmd.query));
      const prompts: unknown[] = [];
      let settled = false;
      const run = m.startSearch().finally(() => {
        settled = true;
      });
      while (!settled) {
        const top = useDialogStore.getState().stack.at(-1);
        if (top?.name === "password") {
          prompts.push({ fileName: top.props.fileName, wrong: top.props.wrong });
          if (cmd.password !== undefined && prompts.length === 1) {
            (top.props.resolve as (v: string | null) => void)(String(cmd.password));
          } else {
            useDialogStore.getState().close("password");
          }
        }
        await new Promise((r) => setTimeout(r, 25));
      }
      await run;
      await new Promise((r) => setTimeout(r, 300));
      const after = await api.engineStats();
      const s = m.useMultiSearch.getState();
      return {
        phase: s.phase,
        prompts,
        files: s.files.map((f) => ({
          name: f.name, status: f.status, hits: f.hits.length, reasonKey: f.reasonKey, detail: f.detail,
          pages: [...new Set(f.hits.map((h) => h.page))].slice(0, 12),
          first: f.hits[0]?.context,
        })),
        summary: m.summarize(s.files),
        engineDocsBefore: before.docs,
        engineDocsAfter: after.docs,
        toasts: renderedToasts(0).slice(-3),
      };
    }
    /**
     * v0.3 real-app QA: call one exported function of a fixed set of modules (still a table, not
     * an `eval`), e.g. `{op:"call", mod:"edit/redact", fn:"findAndMark", args:[…]}`. A store module
     * takes `fn:"getState"` plus an optional `pick` (a key of the state) or `method` + `args`.
     */
    case "call": {
      const loaders: Record<string, () => Promise<unknown>> = {
        "edit/redact": () => import("../edit/redact"),
        "edit/actions": () => import("../edit/actions"),
        "edit/editStore": () => import("../edit/editStore"),
        "forms/formActions": () => import("../forms/formActions"),
        "forms/formStore": () => import("../forms/formStore"),
        "annot/actions": () => import("../annot/actions"),
        "batch/flow": () => import("../batch/flow"),
        "print/printFlow": () => import("../print/printFlow"),
        "print/printStore": () => import("../print/printStore"),
        "tools/snapshot": () => import("../tools/snapshot"),
        "tts/speak": () => import("../tts/speak"),
        "tts/ttsStore": () => import("../tts/ttsStore"),
        "dialogs/flows": () => import("../dialogs/flows"),
        "dialogs/imagesFlow": () => import("../dialogs/imagesFlow"),
        "store/viewStore": () => Promise.resolve({ useViewStore }),
        "store/appStore": () => Promise.resolve({ useAppStore }),
        "store/docStore": () => Promise.resolve({ useDocStore }),
        "dialogs/dialogState": () => Promise.resolve({ useDialogStore }),
      };
      const load = loaders[String(cmd.mod)];
      if (!load) throw new Error(`unknown module ${String(cmd.mod)}`);
      const m = (await load()) as Record<string, unknown>;
      const target = m[String(cmd.fn)];
      if (target && typeof target === "function" && "getState" in target) {
        const st = (target as { getState(): Record<string, unknown> }).getState();
        if (cmd.method) {
          const out = await (st[String(cmd.method)] as (...a: unknown[]) => unknown)(...((cmd.args as unknown[]) ?? []));
          await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 200)));
          return { out: safeClone(out) };
        }
        return safeClone(cmd.pick ? st[String(cmd.pick)] : st);
      }
      if (typeof target !== "function") throw new Error(`unknown function ${String(cmd.mod)}.${String(cmd.fn)}`);
      // prompts that open while the call runs are answered in order from `answers` (then cancelled)
      const answers = ((cmd.answers as unknown[]) ?? []).slice();
      const prompts: unknown[] = [];
      const lastToast = Math.max(0, ...useToastStore.getState().toasts.map((x) => x.id));
      let settled = false;
      const call = Promise.resolve((target as (...a: unknown[]) => unknown)(...((cmd.args as unknown[]) ?? []))).finally(() => {
        settled = true;
      });
      while (!settled) {
        const top = useDialogStore.getState().stack.at(-1);
        const p = top?.props as Record<string, unknown> | undefined;
        if (top && p && typeof p.resolve === "function" && !p.__qaAnswered) {
          p.__qaAnswered = true;
          const texts: Record<string, string> = {};
          for (const k of ["titleKey", "bodyKey", "hintKey", "confirmKey"]) {
            if (typeof p[k] === "string") texts[k] = t(p[k] as string, (p.bodyParams ?? undefined) as TParams);
          }
          const answer = answers.length ? answers.shift() : undefined;
          prompts.push({ name: top.name, texts, answer: answer ?? null });
          if (answer === undefined) useDialogStore.getState().close(top.name);
          else (p.resolve as (v: unknown) => void)(answer);
        }
        await new Promise((r) => setTimeout(r, 25));
      }
      const out = await call;
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 300)));
      return { out: safeClone(out), prompts, toasts: renderedToasts(lastToast).slice(-6) };
    }
    /** v0.3 real-app QA: the top dialog (name + rendered prompt text); `answer` resolves a prompt. */
    case "dialog": {
      const top = useDialogStore.getState().stack.at(-1);
      if (!top) return { top: null };
      const p = top.props as Record<string, unknown>;
      const text: Record<string, string> = {};
      for (const k of ["titleKey", "bodyKey", "hintKey", "confirmKey", "messageKey"]) {
        if (typeof p[k] === "string") text[k] = t(p[k] as string, (p[k.replace("Key", "Params")] ?? p.bodyParams) as TParams);
      }
      if ("answer" in cmd && typeof p.resolve === "function") {
        (p.resolve as (v: unknown) => void)(cmd.answer);
        await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 300)));
      }
      const dom = document.querySelector(".dialog, [role=dialog], [role=alertdialog]");
      return { top: top.name, stack: useDialogStore.getState().stack.map((d) => d.name), text, domText: dom?.textContent?.slice(0, 600) ?? null };
    }
    /** v0.3 real-app QA: click the first element matching `selector` (and containing `text`). */
    case "click": {
      const all = [...document.querySelectorAll<HTMLElement>(String(cmd.selector ?? "button"))];
      const el = cmd.text ? all.find((e) => (e.textContent ?? "").includes(String(cmd.text)) || e.getAttribute("aria-label")?.includes(String(cmd.text))) : all[0];
      if (!el) throw new Error(`no element ${String(cmd.selector)} ${String(cmd.text ?? "")}`);
      el.click();
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 400)));
      return { clicked: el.textContent?.slice(0, 80) ?? el.tagName, disabled: (el as HTMLButtonElement).disabled ?? null, toasts: renderedToasts(Number(cmd.toastsAfter ?? 0)).slice(-4), dialogs: useDialogStore.getState().stack.map((d) => d.name) };
    }
    /** v0.3 real-app QA: text / attributes of the elements matching `selector`. */
    case "dom": {
      const all = [...document.querySelectorAll<HTMLElement>(String(cmd.selector))];
      return {
        count: all.length,
        items: all.slice(0, Number(cmd.limit ?? 10)).map((e) => {
          const r = e.getBoundingClientRect();
          return { tag: e.tagName, cls: e.className, text: e.textContent?.slice(0, Number(cmd.chars ?? 120)), title: e.getAttribute("title"), aria: e.getAttribute("aria-label"), disabled: (e as HTMLButtonElement).disabled ?? null, value: (e as HTMLInputElement).value ?? null, img: e instanceof HTMLImageElement ? { complete: e.complete, w: e.naturalWidth, src: e.src.slice(0, 200) } : undefined, rect: [Math.round(r.left), Math.round(r.top), Math.round(r.width), Math.round(r.height)] };
        }),
      };
    }
    /** v0.3 real-app QA: fetch a URL from the page (e.g. a `seepdf://` render) — status and a text head. */
    case "fetch": {
      const res = await fetch(String(cmd.url), { cache: "no-store" });
      const type = res.headers.get("content-type") ?? "";
      const body = type.startsWith("text") ? (await res.text()).slice(0, 300) : `${(await res.arrayBuffer()).byteLength} bytes`;
      return { status: res.status, type, body, xcache: res.headers.get("x-cache") };
    }
    /** v0.3 real-app QA: set a form control's value the way typing / picking would (React sees it). */
    case "setValue": {
      const el = document.querySelector<HTMLInputElement | HTMLSelectElement | HTMLTextAreaElement>(String(cmd.selector));
      if (!el) throw new Error(`no element ${String(cmd.selector)}`);
      const proto = el instanceof HTMLSelectElement ? HTMLSelectElement.prototype : el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
      if (el instanceof HTMLInputElement && (el.type === "checkbox" || el.type === "radio")) {
        if (el.checked !== Boolean(cmd.value)) el.click();
      } else {
        Object.getOwnPropertyDescriptor(proto, "value")?.set?.call(el, String(cmd.value));
        el.dispatchEvent(new Event("input", { bubbles: true }));
        el.dispatchEvent(new Event("change", { bubbles: true }));
      }
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 300)));
      return { value: (el as HTMLInputElement).value, checked: (el as HTMLInputElement).checked ?? null };
    }
    /** v0.3 real-app QA: a pointer drag on the element under client point `from` to `to`. */
    case "drag": {
      const [x0, y0] = cmd.from as [number, number];
      const [x1, y1] = cmd.to as [number, number];
      const target = (cmd.selector ? document.querySelector<HTMLElement>(String(cmd.selector)) : document.elementFromPoint(x0, y0)) as HTMLElement | null;
      if (!target) throw new Error("nothing under the start point");
      const base = { bubbles: true, cancelable: true, composed: true, pointerId: Number(cmd.pointerId ?? 1), pointerType: "mouse", isPrimary: true, button: 0 };
      target.dispatchEvent(new PointerEvent("pointerdown", { ...base, clientX: x0, clientY: y0, buttons: 1 }));
      const steps = Number(cmd.steps ?? 8);
      for (let i = 1; i <= steps; i++) {
        const x = x0 + ((x1 - x0) * i) / steps;
        const y = y0 + ((y1 - y0) * i) / steps;
        (document.elementFromPoint(x, y) ?? target).dispatchEvent(new PointerEvent("pointermove", { ...base, clientX: x, clientY: y, buttons: 1 }));
        await new Promise((r) => setTimeout(r, 16));
      }
      (document.elementFromPoint(x1, y1) ?? target).dispatchEvent(new PointerEvent("pointerup", { ...base, clientX: x1, clientY: y1, buttons: 0 }));
      await new Promise((r) => setTimeout(r, Number(cmd.settleMs ?? 500)));
      return { target: `${target.tagName}.${target.className}`, toasts: renderedToasts(Number(cmd.toastsAfter ?? 0)).slice(-4) };
    }
    case "closeDialogs":
      useDialogStore.getState().closeAll();
      return snapshot();
    case "sleep":
      await new Promise((r) => setTimeout(r, Number(cmd.ms ?? 500)));
      return { slept: cmd.ms ?? 500 };
    default:
      throw new Error(`unknown op ${cmd.op}`);
  }
}

function scrollerOf(pane: string): HTMLElement {
  const el = document.querySelector<HTMLElement>(`.viewer[data-pane="${pane}"]`);
  if (!el) throw new Error(`no pane ${pane}`);
  return el;
}

/** The page whose box covers the scroller's top edge (+ a few pixels), per pane in the DOM. */
function panesSnapshot(): unknown {
  const v = useViewStore.getState();
  const dom = [...document.querySelectorAll<HTMLElement>(".viewer[data-pane]")].map((el) => {
    const top = el.getBoundingClientRect().top + 40;
    let atTop: number | null = null;
    for (const shell of el.querySelectorAll<HTMLElement>(".page-shell[data-page]")) {
      const r = shell.getBoundingClientRect();
      if (r.top <= top && r.bottom > top) {
        atTop = Number(shell.dataset.page);
        break;
      }
    }
    return { pane: el.dataset.pane, focused: el.dataset.focused !== undefined, scrollTop: Math.round(el.scrollTop), pageAtTop: atTop };
  });
  return {
    split: v.split,
    focusedPane: v.focusedPane,
    main: paneView(v, "main").currentPage,
    second: v.split ? paneView(v, "second").currentPage : null,
    dom,
  };
}

let started = false;
let stopped = false;

/** Ring buffer behind the `console` op: `console.error` / `console.warn` and uncaught errors. */
const consoleLog: { level: string; text: string; at: number }[] = [];
function captureConsole(): void {
  const push = (level: string, args: unknown[]) => {
    const text = args
      .map((a) => (a instanceof Error ? `${a.name}: ${a.message}` : typeof a === "string" ? a : safeJson(a)))
      .join(" ")
      .slice(0, 600);
    consoleLog.push({ level, text, at: Math.round(performance.now()) });
    if (consoleLog.length > 500) consoleLog.shift();
  };
  for (const level of ["error", "warn"] as const) {
    const original = console[level].bind(console);
    console[level] = (...args: unknown[]) => {
      push(level, args);
      original(...args);
    };
  }
  window.addEventListener("error", (e) => push("uncaught", [e.error ?? e.message]));
  window.addEventListener("unhandledrejection", (e) => push("unhandledrejection", [e.reason]));
}
function safeClone(v: unknown): unknown {
  try {
    return JSON.parse(JSON.stringify(v ?? null, (_k, x) => (typeof x === "function" ? undefined : x)));
  } catch {
    return String(v);
  }
}
function safeJson(v: unknown): string {
  try {
    return JSON.stringify(v);
  } catch {
    return String(v);
  }
}

/**
 * Poll the dev server for work. Started once, from `App.tsx`, only in dev.
 *
 * NOTE for whoever drives a smoke run: **do not edit `src/**` while it is in flight.** Vite's
 * HMR re-evaluates the changed module *and its importers*, which gives zustand a second store
 * instance — the UI keeps the new one, a closure captured before the update keeps the old one,
 * and the bridge starts answering from a store with no document. `import.meta.hot.dispose`
 * below at least stops the superseded poller so there is one answer per request.
 */
export function startDevBridge(run: Run): void {
  if (started) return;
  started = true;
  stopped = false;
  captureConsole();
  if (import.meta.hot) {
    import.meta.hot.dispose(() => {
      stopped = true;
      started = false;
    });
  }
  const answer = (id: number, payload: unknown) =>
    void fetch("/__dev/result", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ id, ...(payload as object) }),
    }).catch(() => undefined);

  const tick = async () => {
    try {
      const res = await fetch("/__dev/cmd");
      const batch: { id: number; body: Cmd }[] = await res.json();
      for (const item of batch) {
        try {
          answer(item.id, { ok: true, value: await execute(item.body, run) });
        } catch (e) {
          answer(item.id, { ok: false, error: e instanceof Error ? e.message : String(e), detail: JSON.stringify(e) });
        }
      }
    } catch {
      /* the dev server is not there (production preview) — stay quiet */
    }
    if (!stopped) setTimeout(() => void tick(), 200);
  };
  void tick();
  // eslint-disable-next-line no-console
  console.info("[seepdf] dev bridge active — POST localhost:1420/__dev/cmd");
}
