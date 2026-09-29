import { useLayoutEffect, useRef, useState } from "react";
import { ChevronDown, X } from "lucide-react";
import { useT } from "../i18n/useT";
import { useTabStore } from "../store/tabStore";

/** Everything a tab does beyond drawing itself is in the lazy `tabs/flow.ts`. */
const flow = (fn: (m: typeof import("../tabs/flow")) => unknown) => void import("../tabs/flow").then(fn);

/** Where the dragged tab `id` goes for a pointer at `x`: after every other tab whose centre is left of it. */
export function indexAt(strip: HTMLElement, x: number, id: number): number {
  let at = 0;
  for (const cell of strip.children) {
    if ((cell as HTMLElement).dataset.tab === String(id)) continue;
    const r = cell.getBoundingClientRect();
    if (x > r.left + r.width / 2) at++;
  }
  return at;
}

/**
 * 문서 탭 (v0.3 DR1, UI_SPEC §2.2): the window's documents in the title bar, in place of the
 * document title once there are two or more. Click shows a tab, × / middle click closes it, a drag
 * reorders, right click opens its menu; ⌄ lists every tab when they do not fit.
 */
export function TabStrip() {
  const t = useT();
  const tabs = useTabStore((s) => s.tabs);
  const activeId = useTabStore((s) => s.activeId);
  const strip = useRef<HTMLDivElement>(null);
  const drag = useRef<{ id: number; x: number; moved: boolean } | null>(null);
  const [overflow, setOverflow] = useState(false);

  useLayoutEffect(() => {
    const el = strip.current;
    if (!el) return;
    const check = () => setOverflow(el.scrollWidth > el.clientWidth + 1);
    check();
    el.querySelector<HTMLElement>("[aria-selected=true]")?.scrollIntoView?.({ block: "nearest", inline: "nearest" });
    window.addEventListener("resize", check);
    return () => window.removeEventListener("resize", check);
  }, [tabs, activeId]);

  if (tabs.length < 2) return null;
  return (
    <div className="tabstrip" data-tauri-drag-region="false">
      <div className="tabs" role="tablist" aria-label={t("tabs.label")} ref={strip}>
        {tabs.map((tab) => (
          <div
            key={tab.id}
            data-tab={tab.id}
            role="tab"
            className="tab"
            aria-selected={tab.id === activeId}
            tabIndex={tab.id === activeId ? 0 : -1}
            title={tab.path ?? tab.name}
            onPointerDown={(e) => {
              if (e.button !== 0) return;
              drag.current = { id: tab.id, x: e.clientX, moved: false };
              e.currentTarget.setPointerCapture?.(e.pointerId);
            }}
            onPointerMove={(e) => {
              const d = drag.current;
              if (!d || !strip.current || (!d.moved && Math.abs(e.clientX - d.x) < 6)) return;
              d.moved = true;
              useTabStore.getState().move(d.id, indexAt(strip.current, e.clientX, d.id));
            }}
            onPointerUp={() => {
              const d = drag.current;
              drag.current = null;
              if (d && !d.moved) flow((m) => m.activateTab(tab.id));
            }}
            onPointerCancel={() => (drag.current = null)}
            onMouseDown={(e) => e.button === 1 && e.preventDefault()}
            onAuxClick={(e) => e.button === 1 && flow((m) => m.closeTab(tab.id))}
            onKeyDown={(e) => e.key === "Enter" && flow((m) => m.activateTab(tab.id))}
            onContextMenu={(e) => {
              e.preventDefault();
              const { clientX, clientY } = e;
              flow((m) => m.openTabMenu(tab.id, clientX, clientY));
            }}
          >
            {tab.dirty && <span className="dirty-dot" aria-label={t("app.edited")} />}
            <span className="tab-name">{tab.name}</span>
            <button
              type="button"
              className="tab-close"
              aria-label={t("tabs.close")}
              onPointerDown={(e) => e.stopPropagation()}
              onClick={() => flow((m) => m.closeTab(tab.id))}
            >
              <X size={12} strokeWidth={2} aria-hidden />
            </button>
          </div>
        ))}
      </div>
      {overflow && (
        <button
          type="button"
          className="icon-btn tab-more"
          aria-label={t("tabs.all")}
          onClick={(e) => {
            const r = e.currentTarget.getBoundingClientRect();
            flow((m) => m.openTabList(r.left, r.bottom + 6));
          }}
        >
          <ChevronDown size={14} strokeWidth={1.75} aria-hidden />
        </button>
      )}
    </div>
  );
}
