/**
 * v0.3 pkg3: what the title bar's badges open (lazy chunk).
 *
 * * 서명됨 (S1): every detected signature — field name, 사유, 서명 시각 — the note that nothing was
 *   validated, and whether the next save keeps the signatures (incremental) or rewrites the file.
 * * 제한됨 (S5): the restrictions of this open and 권한 암호로 잠금 해제…, which asks for the
 *   permissions password and reopens the document with it (same docId, same undo history).
 */
import { useEffect, useRef } from "react";
import * as api from "../ipc/api";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { askPassword } from "../dialogs/dialogState";
import { toast } from "./toastStore";
import { restrictions } from "./permissions";
import "./badges.css";

/** `D:20260901120000+09'00'` → `2026-09-01 12:00`; anything else as it is. */
export function formatPdfDate(raw: string): string {
  const m = /^D:(\d{4})(\d{2})?(\d{2})?(\d{2})?(\d{2})?/.exec(raw);
  if (!m) return raw;
  const [, y, mo = "01", d = "01", h, mi] = m;
  return h ? `${y}-${mo}-${d} ${h}:${mi ?? "00"}` : `${y}-${mo}-${d}`;
}

/** 권한 암호로 잠금 해제: the password prompt, retried in place until right or cancelled. */
export async function unlockFlow(): Promise<boolean> {
  const info = useDocStore.getState().info;
  if (!info) return false;
  let wrong = false;
  for (;;) {
    const password = await askPassword(info.name, wrong);
    if (password === null) return false;
    try {
      const unlocked = await api.unlockDocument({ docId: info.docId, password });
      useDocStore.getState().adopt(unlocked);
      toast("security.restricted.unlocked", undefined, { tone: "success" });
      return true;
    } catch (e) {
      if (api.isSeePdfError(e) && e.code === "passwordWrong") {
        wrong = true;
        continue;
      }
      toast("error.generic", undefined, { tone: "danger", detail: e instanceof Error ? e.message : String(e) });
      return false;
    }
  }
}

export default function BadgePopover({ which, onClose }: { which: "signed" | "restricted"; onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    const onDown = (e: PointerEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node) && !(e.target as HTMLElement).closest?.(".doc-badge")) {
        onClose();
      }
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown);
    };
  }, [onClose]);

  if (!info) return null;
  const title = t(which === "signed" ? "security.signed.title" : "security.restricted.title");
  return (
    <div ref={ref} className="badge-pop" role="dialog" aria-label={title}>
      <h3 className="text-base badge-pop-head">{title}</h3>
      {which === "signed" ? (
        <>
          <ul className="badge-list">
            {(info.signatures ?? []).map((s, i) => (
              <li key={i}>
                <span className="text-base">{s.fieldName ?? t("security.signed.unnamed")}</span>
                {s.reason && <span className="text-xs dim">{t("security.signed.reason", { reason: s.reason })}</span>}
                {s.time && <span className="text-xs dim">{t("security.signed.time", { time: formatPdfDate(s.time) })}</span>}
                {s.subFilter && <span className="text-xs dim mono">{s.subFilter}</span>}
              </li>
            ))}
          </ul>
          <p className="text-xs dim">{t("security.signed.note")}</p>
          <p className="text-xs">
            {t(info.incrementalSave === false ? "security.signed.rewrite" : "security.signed.incremental")}
          </p>
        </>
      ) : (
        <>
          <ul className="badge-list">
            {restrictions(info).map((k) => (
              <li key={k} className="text-base">
                {t(`security.restricted.${k}`)}
              </li>
            ))}
          </ul>
          <button
            type="button"
            className="btn"
            onClick={() => {
              onClose();
              void unlockFlow();
            }}
          >
            {t("security.restricted.unlock")}
          </button>
        </>
      )}
    </div>
  );
}
