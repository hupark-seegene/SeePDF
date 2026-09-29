/**
 * Context menus (UI_SPEC §12) — rendered in the webview, themed and localised, never native.
 *
 * `openContextMenu({ x, y, items })` from `contextMenuStore.ts` is the only entry point; the store
 * is entry-resident and this renderer is mounted lazily while a menu is open.
 */
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useT } from "../i18n/useT";
import { closeContextMenu, isSeparator, useContextMenuStore } from "./contextMenuStore";
import "./overlays.css";

const MARGIN = 8;

export default function ContextMenu() {
  const t = useT();
  const menu = useContextMenuStore((s) => s.menu);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ x: 0, y: 0 });
  const [active, setActive] = useState(0);

  // keep the menu inside the window
  useLayoutEffect(() => {
    if (!menu) return;
    const el = ref.current;
    const w = el?.offsetWidth ?? 200;
    const h = el?.offsetHeight ?? 160;
    setPos({
      x: Math.max(MARGIN, Math.min(menu.x, window.innerWidth - w - MARGIN)),
      y: Math.max(MARGIN, Math.min(menu.y, window.innerHeight - h - MARGIN)),
    });
    setActive(0);
    el?.focus();
  }, [menu]);

  useEffect(() => {
    if (!menu) return;
    const close = () => closeContextMenu();
    window.addEventListener("resize", close);
    window.addEventListener("blur", close);
    document.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("resize", close);
      window.removeEventListener("blur", close);
      document.removeEventListener("scroll", close, true);
    };
  }, [menu]);

  if (!menu) return null;
  const items = menu.items;
  const selectable = items.map((item, i) => (isSeparator(item) || item.disabled ? -1 : i)).filter((i) => i >= 0);

  const step = (delta: number) => {
    const at = selectable.indexOf(active);
    const next = selectable[(at + delta + selectable.length) % selectable.length] ?? selectable[0];
    setActive(next ?? 0);
  };

  const choose = (index: number) => {
    const item = items[index];
    if (!item || isSeparator(item) || item.disabled) return;
    closeContextMenu();
    item.onSelect?.();
  };

  return (
    <div className="menu-layer" onPointerDown={() => closeContextMenu()} onContextMenu={(e) => e.preventDefault()}>
      <div
        className="ctx-menu"
        role="menu"
        aria-label={t(menu.labelKey ?? "common.more")}
        tabIndex={-1}
        ref={ref}
        style={{ left: pos.x, top: pos.y }}
        onPointerDown={(e) => e.stopPropagation()}
        onKeyDown={(e) => {
          if (e.key === "Escape") return closeContextMenu();
          if (e.key === "ArrowDown") { e.preventDefault(); step(1); }
          if (e.key === "ArrowUp") { e.preventDefault(); step(-1); }
          if (e.key === "Enter" || e.key === " ") { e.preventDefault(); choose(active); }
        }}
      >
        {items.map((item, i) =>
          isSeparator(item) ? (
            <span className="ctx-sep" key={item.id} role="separator" />
          ) : (
            <button
              key={item.id}
              type="button"
              role="menuitem"
              className="ctx-item"
              data-danger={item.danger || undefined}
              data-active={active === i || undefined}
              disabled={item.disabled}
              title={item.hintKey ? t(item.hintKey) : undefined}
              onMouseEnter={() => setActive(i)}
              onClick={() => choose(i)}
            >
              <span>{item.label ?? t(item.labelKey)}</span>
              {item.shortcut && <span className="ctx-key text-xs dim">{item.shortcut}</span>}
            </button>
          ),
        )}
      </div>
    </div>
  );
}
