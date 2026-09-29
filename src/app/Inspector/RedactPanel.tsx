/**
 * UI_SPEC §7 "영역 표시 pending" (F-22): 채우기 색상 (기본 검정), 덮어쓸 문구, 표시된 영역 {{count}}개,
 * the live `redact_preview` per marked page — what goes, the collateral text in amber with why —
 * and **적용** (destructive; `applyMarks` confirms), disabled with the reason when a mark covers a
 * form field. Styles live in `edit/edit.css` (this panel only shows in 편집 mode).
 *
 * v0.3 (pkg1): 검색해서 표시 (R3) — a keyword and the 개인정보 patterns over every page (or the current
 * page, or a range) become labelled marks; 표시 목록 lists them for review, each removable before 적용.
 * The preview says per text run whether only the marked characters go (R2) and per image whether
 * its pixels are blanked or it goes whole (R1); marks inside a group are ungrouped on 적용 (R4).
 */
import { useRef, useState } from "react";
import { X } from "lucide-react";
import { useT } from "../../i18n/useT";
import type { PageIndex, Rgb } from "../../ipc/types";
import { useDocStore } from "../../store/docStore";
import { useViewStore } from "../../store/viewStore";
import { parsePageRange } from "../../dialogs/pageRange";
import { useEditStore, type RedactPreviewState } from "../../edit/editStore";
import { applyMarks, dropMarks, findAndMark, markedPages, markGroups, type MarkSource } from "../../edit/redact";
import { PATTERN_KINDS, type PatternKind } from "../../edit/redactPatterns";
import { rgbCss } from "../Swatches";

const FILLS: { key: string; rgb: Rgb }[] = [
  { key: "color.black", rgb: [0, 0, 0] },
  { key: "color.gray", rgb: [128, 128, 128] },
  { key: "color.white", rgb: [255, 255, 255] },
];

export function RedactPanel() {
  const t = useT();
  const marks = useEditStore((s) => s.marks);
  const previews = useEditStore((s) => s.previews);
  const fill = useEditStore((s) => s.redactFill);
  const overlay = useEditStore((s) => s.redactOverlay);
  const setOptions = useEditStore((s) => s.setRedactOptions);
  const [busy, setBusy] = useState(false);

  const pages = markedPages(marks);
  const fields = pages.flatMap((p) => {
    const state = previews[p];
    return state?.status === "ready" ? state.result.formFields : [];
  });
  const blockedReason = fields.length ? t("redact.formFields", { names: fields.join(", ") }) : undefined;

  const apply = async () => {
    setBusy(true);
    try {
      await applyMarks();
    } finally {
      setBusy(false);
    }
  };

  return (
    <section className="field-group redact-panel" aria-label={t("redact.title")}>
      <h3 className="field-label text-xs">{t("redact.title")}</h3>
      <p className="text-sm" data-testid="redact-count">
        {marks.length ? t("redact.marked", { count: marks.length }) : t("redact.empty")}
      </p>

      <h3 className="field-label text-xs">{t("prop.fillColor")}</h3>
      <div className="swatches redact-fills" role="radiogroup" aria-label={t("prop.fillColor")}>
        {FILLS.map((f) => {
          const on = f.rgb.join() === fill.join();
          return (
            <button
              key={f.key}
              type="button"
              role="radio"
              aria-checked={on}
              aria-label={t("a11y.colorSwatch", { name: t(f.key) })}
              className="swatch"
              data-selected={on || undefined}
              style={{ background: rgbCss(f.rgb) }}
              onClick={() => setOptions({ fill: f.rgb })}
            />
          );
        })}
      </div>

      <h3 className="field-label text-xs">
        <label htmlFor="redact-overlay">{t("redact.overlayText")}</label>
      </h3>
      <input
        id="redact-overlay"
        className="field"
        type="text"
        value={overlay}
        placeholder={t("redact.overlayPlaceholder")}
        onChange={(e) => setOptions({ overlay: e.currentTarget.value })}
      />

      <FindSection />

      {marks.length > 0 && <ReviewList />}

      {pages.length > 0 && <h3 className="field-label text-xs">{t("redact.previewTitle")}</h3>}
      {pages.map((p) => (
        <PagePreview key={p} page={p} state={previews[p]} />
      ))}

      {blockedReason && (
        <p className="banner danger text-sm" role="alert">
          {blockedReason}
        </p>
      )}

      <div className="redact-actions">
        <button
          type="button"
          className="btn redact-apply"
          disabled={marks.length === 0 || !!blockedReason || busy}
          title={blockedReason}
          onClick={() => void apply()}
        >
          {t("redact.apply")}
        </button>
        {marks.length > 0 && (
          <button type="button" className="btn quiet" disabled={busy} onClick={dropMarks}>
            {t("redact.clearAll")}
          </button>
        )}
      </div>
    </section>
  );
}

type Scope = "all" | "current" | "range";

/** v0.3 (R3) 검색해서 표시: keyword + 개인정보 patterns → labelled marks on every page in scope. */
function FindSection() {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const [keyword, setKeyword] = useState("");
  const [patterns, setPatterns] = useState<PatternKind[]>([]);
  const [scope, setScope] = useState<Scope>("all");
  const [range, setRange] = useState("");
  const [progress, setProgress] = useState<{ done: number; total: number } | null>(null);
  const [result, setResult] = useState<{ key: string; count?: number } | null>(null);
  const signal = useRef<{ cancelled: boolean } | null>(null);

  const pageCount = info?.pageCount ?? 0;
  const pagesInScope = (): PageIndex[] | null => {
    if (scope === "current") return [useViewStore.getState().currentPage];
    if (scope === "range") return parsePageRange(range, pageCount);
    return Array.from({ length: pageCount }, (_, i) => i);
  };
  const nothing = !keyword.trim() && patterns.length === 0;

  const run = async () => {
    const pages = pagesInScope();
    if (nothing) {
      setResult({ key: "redact.find.needInput" });
      return;
    }
    if (!pages) {
      setResult({ key: "redact.find.badRange" });
      return;
    }
    const token = { cancelled: false };
    signal.current = token;
    setResult(null);
    setProgress({ done: 0, total: pages.length });
    try {
      const count = await findAndMark({ keyword, patterns, pages }, (done, total) => setProgress({ done, total }), token);
      setResult(count ? { key: "redact.find.found", count } : { key: "redact.find.none" });
    } finally {
      signal.current = null;
      setProgress(null);
    }
  };

  const toggle = (kind: PatternKind, on: boolean) =>
    setPatterns((prev) => (on ? [...prev, kind] : prev.filter((k) => k !== kind)));

  return (
    <div className="redact-find" role="group" aria-labelledby="redact-find-title">
      <h3 id="redact-find-title" className="field-label text-xs">
        {t("redact.find.title")}
      </h3>
      <input
        className="field"
        type="search"
        value={keyword}
        aria-label={t("redact.find.keyword")}
        placeholder={t("redact.find.keywordPlaceholder")}
        autoComplete="off"
        spellCheck={false}
        onChange={(e) => setKeyword(e.currentTarget.value)}
        onKeyDown={(e) => {
          if (e.key !== "Enter" || e.nativeEvent.isComposing || e.keyCode === 229) return;
          e.preventDefault();
          if (!progress) void run();
        }}
      />
      <fieldset className="redact-find-patterns">
        <legend className="text-xs dim">{t("redact.find.patterns")}</legend>
        {PATTERN_KINDS.map((kind) => (
          <label key={kind} className="search-toggle text-sm">
            <input type="checkbox" checked={patterns.includes(kind)} onChange={(e) => toggle(kind, e.currentTarget.checked)} />
            {t(`redact.find.${kind}`)}
          </label>
        ))}
      </fieldset>
      <div className="redact-find-scope" role="radiogroup" aria-label={t("redact.find.scope")}>
        {(["all", "current", "range"] as const).map((s) => (
          <label key={s} className="search-toggle text-sm">
            <input type="radio" name="redact-find-scope" checked={scope === s} onChange={() => setScope(s)} />
            {t(`redact.find.scope.${s}`)}
          </label>
        ))}
        {scope === "range" && (
          <input
            className="field"
            type="text"
            value={range}
            aria-label={t("redact.find.scope.range")}
            placeholder={t("redact.find.rangePlaceholder")}
            onChange={(e) => setRange(e.currentTarget.value)}
          />
        )}
      </div>
      <div className="redact-actions">
        {progress ? (
          <>
            <span className="text-sm dim" role="status">
              {t("redact.find.running", { done: progress.done, total: progress.total })}
            </span>
            <button type="button" className="btn quiet" onClick={() => signal.current && (signal.current.cancelled = true)}>
              {t("common.cancel")}
            </button>
          </>
        ) : (
          <button type="button" className="btn" disabled={!info || nothing} onClick={() => void run()}>
            {t("redact.find.run")}
          </button>
        )}
      </div>
      {result && (
        <p className="text-sm dim" role="status" data-testid="redact-find-result">
          {result.count !== undefined ? t(result.key, { count: result.count }) : t(result.key)}
        </p>
      )}
    </div>
  );
}

const SOURCE_KEY: Record<MarkSource, string> = {
  keyword: "redact.find.keyword",
  rrn: "redact.find.rrn",
  phone: "redact.find.phone",
  email: "redact.find.email",
  account: "redact.find.account",
  search: "redact.review.search",
  area: "redact.review.area",
};

/** 표시 목록: every pending mark (a match's marks as one entry), click → go there, × → remove. */
function ReviewList() {
  const t = useT();
  const marks = useEditStore((s) => s.marks);
  const groups = markGroups(marks);
  return (
    <div className="redact-review">
      <h3 className="field-label text-xs">{t("redact.review.title")}</h3>
      <ul className="redact-review-list" aria-label={t("redact.review.title")}>
        {groups.map((g) => (
          <li key={g.key} data-source={g.source}>
            <button
              type="button"
              className="redact-review-item text-sm"
              onClick={() => {
                useViewStore.getState().goToPage(g.page);
                useEditStore.getState().selectMark(g.ids[0]);
              }}
            >
              <span className="dim">{t(SOURCE_KEY[g.source])}</span>
              {g.text && <span className="redact-review-text">{g.text}</span>}
              <span className="dim mono text-xs">{t("redact.pageLabel", { page: g.page + 1 })}</span>
            </button>
            <button
              type="button"
              className="icon-btn redact-review-remove"
              aria-label={t("redact.review.remove")}
              title={t("redact.review.remove")}
              onClick={() => useEditStore.getState().removeMarks(g.ids)}
            >
              <X size={12} strokeWidth={1.75} aria-hidden />
            </button>
          </li>
        ))}
      </ul>
    </div>
  );
}

function PagePreview({ page, state }: { page: PageIndex; state: RedactPreviewState | undefined }) {
  const t = useT();
  if (!state || state.status === "loading") {
    return (
      <p className="redact-page text-sm dim">
        {t("redact.pageLabel", { page: page + 1 })} · {t("redact.previewLoading")}
      </p>
    );
  }
  if (state.status === "error") {
    return (
      <p className="redact-page text-sm dim">
        {t("redact.pageLabel", { page: page + 1 })} · {t("redact.previewFailed")}
      </p>
    );
  }
  const r = state.result;
  const groups = r.groups?.length ?? 0;
  return (
    <div className="redact-page" data-testid={`redact-preview-${page}`}>
      <p className="text-sm">
        {t("redact.pageSummary", {
          page: page + 1,
          text: r.textObjects.length,
          images: r.imageObjects.length,
          annots: r.annotations.length,
        })}
      </p>
      {r.textObjects.length > 0 && (
        <ul className="redact-list text-xs">
          {r.textObjects.map((o) => (
            <li key={o.objectId} data-collateral={(!o.fullyInside && !o.split) || undefined} data-split={o.split || undefined}>
              {o.text || "—"}
            </li>
          ))}
        </ul>
      )}
      {r.textObjects.some((o) => o.split) && <p className="text-xs dim">{t("redact.splitHint")}</p>}
      {r.imageObjects.length > 0 && (
        <ul className="redact-list text-xs" data-testid={`redact-images-${page}`}>
          {r.imageObjects.map((o) => (
            <li key={o.objectId} data-blank={o.blank || undefined}>
              {t(o.blank ? "redact.imageBlank" : "redact.imageRemove")}
            </li>
          ))}
        </ul>
      )}
      {groups > 0 && (
        <p className="text-xs redact-groups-note" role="note">
          {t("redact.groups.note", { count: groups })}
        </p>
      )}
      {r.collateral.length > 0 && (
        <div className="redact-collateral-note text-xs" role="note">
          <strong>{t("redact.collateral")}</strong>
          <ul>
            {r.collateral.map((text, i) => (
              <li key={i}>{text}</li>
            ))}
          </ul>
          <span className="dim">{t("redact.collateralHint")}</span>
        </div>
      )}
    </div>
  );
}
