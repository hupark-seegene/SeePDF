/**
 * A link's target (P2): 페이지로 이동 — a page (its label or its number) and optionally the current
 * view's position (현재 위치 사용) — or 웹 주소. Used by the popover after a 링크 drag and by the
 * inspector's link section.
 *
 * On an encrypted document 페이지로 이동 is disabled with the reason: a /Dest is written with lopdf,
 * which would have to re-encrypt the file (the engine answers `unsupported`).
 */
import { useId, useState } from "react";
import type { LinkTarget } from "../ipc/types";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { currentViewTarget } from "../store/viewStore";
import { displayLabel, pageForEntry } from "../viewer/pageLabel";
import { normalizeUrl } from "./linkActions";
import "./link.css";

type Kind = "page" | "url";

export function LinkTargetForm({ initial, submitKey, onSubmit, onCancel, autoFocus = false }: {
  initial?: LinkTarget;
  submitKey: string;
  onSubmit(target: LinkTarget): void;
  onCancel?(): void;
  autoFocus?: boolean;
}) {
  const t = useT();
  const id = useId();
  const info = useDocStore((s) => s.info);
  const labels = info?.pageLabels;
  const pageCount = info?.pageCount ?? 0;
  const encrypted = info?.encrypted ?? false;
  const start = initial && "url" in initial ? null : initial;
  const [kind, setKind] = useState<Kind>(initial && "url" in initial ? "url" : encrypted ? "url" : "page");
  const [pageText, setPageText] = useState(() => displayLabel(labels, start?.page ?? currentViewTarget().page));
  // a position kept from the initial target or taken with 현재 위치 사용; typing a page drops it
  const [position, setPosition] = useState<{ page: number; x?: number; y?: number; zoom?: number } | null>(
    start && (start.y !== undefined || start.x !== undefined || start.zoom !== undefined) ? { ...start } : null,
  );
  const [url, setUrl] = useState(initial && "url" in initial ? initial.url : "");

  const page = pageForEntry(pageText, labels, pageCount);
  const normalized = normalizeUrl(url);
  const valid = kind === "page" ? page !== null && !encrypted : normalized.length > 0;

  function takeCurrentView() {
    const view = currentViewTarget();
    setPageText(displayLabel(labels, view.page));
    setPosition(view.y !== undefined ? { page: view.page, y: view.y } : null);
  }

  function submit() {
    if (!valid) return;
    if (kind === "url") return onSubmit({ url: normalized });
    const target = position && position.page === page ? { ...position, page } : { page: page! };
    onSubmit(target);
  }

  return (
    <form
      className="link-form"
      onSubmit={(e) => {
        e.preventDefault();
        submit();
      }}
      onKeyDown={(e) => {
        if (e.key === "Escape" && onCancel) {
          e.stopPropagation();
          onCancel();
        }
      }}
    >
      <div className="segmented small link-kind" role="radiogroup" aria-label={t("link.target")}>
        {(["page", "url"] as const).map((k) => (
          <button
            key={k}
            type="button"
            role="radio"
            aria-checked={kind === k}
            className="segment"
            data-active={kind === k || undefined}
            onClick={() => setKind(k)}
          >
            {t(k === "page" ? "link.kind.page" : "link.kind.url")}
          </button>
        ))}
      </div>

      {kind === "page" ? (
        <div className="link-field">
          <label className="text-xs dim" htmlFor={`${id}-page`}>
            {t("link.page")}
          </label>
          <div className="link-row">
            <input
              id={`${id}-page`}
              className="field link-page-input mono"
              value={pageText}
              autoFocus={autoFocus}
              disabled={encrypted}
              aria-invalid={page === null || undefined}
              onChange={(e) => {
                setPageText(e.target.value);
                setPosition(null);
              }}
            />
            <span className="text-xs dim">/ {pageCount}</span>
            <button type="button" className="btn quiet link-here" disabled={encrypted} onClick={takeCurrentView}>
              {t("link.useCurrent")}
            </button>
          </div>
          <p className="text-xs dim link-hint">
            {encrypted
              ? t("structure.encrypted")
              : position && position.page === page && position.y !== undefined
                ? t("link.position", { y: Math.round(position.y) })
                : t("link.positionTop")}
          </p>
        </div>
      ) : (
        <div className="link-field">
          <label className="text-xs dim" htmlFor={`${id}-url`}>
            {t("link.url")}
          </label>
          <input
            id={`${id}-url`}
            className="field"
            type="text"
            inputMode="url"
            placeholder="https://"
            value={url}
            autoFocus={autoFocus}
            onChange={(e) => setUrl(e.target.value)}
          />
        </div>
      )}

      <div className="link-actions">
        {onCancel && (
          <button type="button" className="btn quiet" onClick={onCancel}>
            {t("common.cancel")}
          </button>
        )}
        <button type="submit" className="btn primary" disabled={!valid}>
          {t(submitKey)}
        </button>
      </div>
    </form>
  );
}
