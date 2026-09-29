/**
 * v0.3 pkg3: the title bar's document badges — 서명됨 (n) (S1) and 제한됨 (S5). The badges are in
 * the entry chunk (a few hundred bytes); what they open, `BadgePopover`, is code-split.
 */
import { Suspense, lazy, useState } from "react";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { restrictions } from "./permissions";

const BadgePopover = lazy(() => import("./BadgePopover"));

type Which = "signed" | "restricted";

export function DocBadges() {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const [open, setOpen] = useState<Which | null>(null);
  if (!info) return null;
  const signed = info.signatures?.length ?? 0;
  const restricted = restrictions(info).length > 0;
  if (!signed && !restricted) return null;
  const toggle = (w: Which) => setOpen((o) => (o === w ? null : w));
  return (
    <div className="doc-badges" data-tauri-drag-region="false">
      {signed > 0 && (
        <button
          type="button"
          className="doc-badge"
          aria-haspopup="dialog"
          aria-expanded={open === "signed"}
          onClick={() => toggle("signed")}
        >
          {t("security.signed.badge", { count: signed })}
        </button>
      )}
      {restricted && (
        <button
          type="button"
          className="doc-badge warn"
          aria-haspopup="dialog"
          aria-expanded={open === "restricted"}
          onClick={() => toggle("restricted")}
        >
          {t("security.restricted.badge")}
        </button>
      )}
      {open && (
        <Suspense fallback={null}>
          <BadgePopover which={open} onClose={() => setOpen(null)} />
        </Suspense>
      )}
    </div>
  );
}
