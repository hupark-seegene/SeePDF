/**
 * 검색 sidebar (F-07, UI_SPEC §4): query field, 대소문자 구분 / 단어 단위 toggles, a live count
 * while the pass streams in, results with ±40 characters of context grouped by page, and
 * Enter / ⇧Enter (= ⌘G / ⇧⌘G) to walk the hits. Esc clears.
 *
 * The streaming itself is `SearchController`; this file is the panel.
 */
import { useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, ChevronUp, X } from "lucide-react";
import { IconButton } from "../app/IconButton";
import { useT } from "../i18n/useT";
import { shortcutFor } from "../keys/keymap";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { useSearchStore } from "../viewer/search/SearchController";
import type { SearchHit } from "../ipc/types";
import "./sidebar.css";

const DEBOUNCE_MS = 220;
const RENDER_CHUNK = 200;

export function SearchPanel() {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);

  const query = useSearchStore((s) => s.query);
  const matchCase = useSearchStore((s) => s.matchCase);
  const wholeWord = useSearchStore((s) => s.wholeWord);
  const hits = useSearchStore((s) => s.hits);
  const current = useSearchStore((s) => s.current);
  const running = useSearchStore((s) => s.running);
  const run = useSearchStore((s) => s.run);
  const step = useSearchStore((s) => s.step);
  const select = useSearchStore((s) => s.select);
  const setOptions = useSearchStore((s) => s.setOptions);
  const clear = useSearchStore((s) => s.clear);

  const [draft, setDraft] = useState(query);
  const [limit, setLimit] = useState(RENDER_CHUNK);
  const inputRef = useRef<HTMLInputElement>(null);
  // The page a *search session* started on: it is captured when the field is empty, so typing
  // "monkey" one character at a time does not walk the first hit forward with every keystroke.
  const fromPageRef = useRef(currentPage);
  if (!query) fromPageRef.current = currentPage;

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  // debounce the field, but re-run the options immediately
  useEffect(() => {
    if (!info) return;
    if (draft === query) return;
    const timer = window.setTimeout(() => {
      setLimit(RENDER_CHUNK);
      void run(info.docId, draft, fromPageRef.current);
    }, DEBOUNCE_MS);
    return () => window.clearTimeout(timer);
  }, [draft, query, info, run]);

  useEffect(() => {
    if (!info || !query) return;
    setLimit(RENDER_CHUNK);
    void run(info.docId, query, fromPageRef.current);
    // re-runs only when an option flips; `query` is handled by the debounce above
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [matchCase, wholeWord]);

  const groups = useMemo(() => groupByPage(hits.slice(0, limit)), [hits, limit]);
  const total = hits.length;

  return (
    <div className="search-panel">
      <div className="search-field">
        <input
          ref={inputRef}
          className="field"
          type="search"
          value={draft}
          autoComplete="off"
          autoCorrect="off"
          spellCheck={false}
          placeholder={t("sidebar.search.placeholder")}
          aria-label={t("sidebar.search.placeholder")}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              if (draft !== query && info) void run(info.docId, draft, fromPageRef.current);
              else step(e.shiftKey ? -1 : 1);
            } else if (e.key === "Escape") {
              e.preventDefault();
              setDraft("");
              clear();
            }
          }}
        />
        {draft && (
          <IconButton
            icon={X}
            label={t("common.close")}
            size={14}
            onClick={() => {
              setDraft("");
              clear();
              inputRef.current?.focus();
            }}
          />
        )}
      </div>

      <div className="search-options">
        <label className="search-toggle text-sm">
          <input
            type="checkbox"
            checked={matchCase}
            onChange={(e) => setOptions({ matchCase: e.target.checked })}
          />
          {t("sidebar.search.matchCase")}
        </label>
        <label className="search-toggle text-sm">
          <input
            type="checkbox"
            checked={wholeWord}
            onChange={(e) => setOptions({ wholeWord: e.target.checked })}
          />
          {t("sidebar.search.wholeWord")}
        </label>
      </div>

      <div className="search-status">
        <span className="text-sm dim" role="status" aria-live="polite">
          {running
            ? t("sidebar.search.searching")
            : query
              ? t("sidebar.search.results", { count: total })
              : ""}
        </span>
        <span className="search-nav">
          <IconButton
            icon={ChevronUp}
            label={t("menu.edit.findPrevious")}
            shortcut={shortcutFor("edit.findPrevious", os)}
            size={14}
            disabled={total === 0}
            onClick={() => step(-1)}
          />
          <IconButton
            icon={ChevronDown}
            label={t("menu.edit.findNext")}
            shortcut={shortcutFor("edit.findNext", os)}
            size={14}
            disabled={total === 0}
            onClick={() => step(1)}
          />
        </span>
      </div>

      {query && !running && total === 0 && <p className="empty">{t("sidebar.search.empty")}</p>}

      <ol className="search-results">
        {groups.map((group) => (
          <li key={group.page}>
            <p className="search-page text-xs dim">{t("sidebar.search.pageLabel", { page: group.page + 1 })}</p>
            <ul>
              {group.hits.map((entry) => (
                <li key={`${entry.hit.page}:${entry.hit.charStart}`}>
                  <button
                    type="button"
                    className="search-result"
                    data-current={entry.index === current || undefined}
                    onClick={() => select(entry.index)}
                  >
                    <Context hit={entry.hit} />
                  </button>
                </li>
              ))}
            </ul>
          </li>
        ))}
      </ol>

      {total > limit && (
        <button type="button" className="btn quiet" onClick={() => setLimit((n) => n + RENDER_CHUNK)}>
          {t("common.more")}
        </button>
      )}
    </div>
  );
}

/** pdfium's page text carries generated control characters; they render as tofu in a list. */
function clean(text: string): string {
  return text.replace(/[\u0000-\u001f\u007f]+/g, " ");
}

function Context({ hit }: { hit: SearchHit }) {
  const [start, length] = hit.contextMatch;
  const before = clean(hit.context.slice(0, start));
  const match = clean(hit.context.slice(start, start + length));
  const after = clean(hit.context.slice(start + length));
  return (
    <span className="text-sm">
      {before}
      <mark>{match}</mark>
      {after}
    </span>
  );
}

function groupByPage(hits: SearchHit[]): { page: number; hits: { hit: SearchHit; index: number }[] }[] {
  const groups: { page: number; hits: { hit: SearchHit; index: number }[] }[] = [];
  hits.forEach((hit, index) => {
    const last = groups[groups.length - 1];
    if (last && last.page === hit.page) last.hits.push({ hit, index });
    else groups.push({ page: hit.page, hits: [{ hit, index }] });
  });
  return groups;
}
