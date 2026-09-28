import { useEffect, useState } from "react";
import { AudioLines, ChevronLeft, ChevronRight, Columns2, Link2, Moon, RotateCcw, RotateCw, Rows2, X, ZoomIn, ZoomOut } from "lucide-react";
import { IconButton } from "./IconButton";
import { resyncPanes } from "../viewer/panes";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { useViewStore, ZOOM_STEPS } from "../store/viewStore";
import { useJobStore } from "../store/jobStore";
import { useAppStore } from "../store/appStore";
import { shortcutFor } from "../keys/keymap";
import { useTtsStore } from "../tts/ttsStore";
import type { ViewLayout } from "../ipc/types";
import { displayLabel, pageForEntry } from "../viewer/pageLabel";

const LAYOUTS: { id: ViewLayout; labelKey: string; keyId: string }[] = [
  { id: "single", labelKey: "view.layout.single", keyId: "view.layout.single" },
  { id: "continuous", labelKey: "view.layout.continuous", keyId: "view.layout.continuous" },
  { id: "two", labelKey: "view.layout.twoPage", keyId: "view.layout.twoPage" },
];

/** 28 px status bar (UI_SPEC §8): page nav · layout · rotate · zoom · save state + progress slot. */
export function StatusBar() {
  const t = useT();
  const os = useAppStore((s) => s.os);
  const info = useDocStore((s) => s.info);
  const { currentPage, zoomPercent, zoomMode, layout } = useViewStore();
  const goToPage = useViewStore((s) => s.goToPage);
  const setLayout = useViewStore((s) => s.setLayout);
  const rotate = useViewStore((s) => s.rotate);
  const zoomIn = useViewStore((s) => s.zoomIn);
  const zoomOut = useViewStore((s) => s.zoomOut);
  const night = useViewStore((s) => s.night);
  const cycleNight = useViewStore((s) => s.cycleNight);
  const setZoom = useViewStore((s) => s.setZoom);
  const setZoomMode = useViewStore((s) => s.setZoomMode);
  const split = useViewStore((s) => s.split);
  const toggleSplit = useViewStore((s) => s.toggleSplit);
  const setSplitOrientation = useViewStore((s) => s.setSplitOrientation);
  const setSyncScroll = useViewStore((s) => s.setSyncScroll);
  const job = useJobStore((s) => s.active);
  const speaking = useTtsStore((s) => s.speaking);
  const cancelJob = useJobStore((s) => s.cancel);

  // P2: with page labels the box shows the current page's label and accepts a label or a number
  const labels = info?.pageLabels;
  const [pageField, setPageField] = useState(displayLabel(labels, currentPage));
  useEffect(() => setPageField(displayLabel(labels, currentPage)), [currentPage, labels]);

  const total = info?.pageCount ?? 0;
  const zoomLabel = zoomMode === "fit-width"
    ? t("view.zoom.fitWidth")
    : zoomMode === "fit-page"
      ? t("view.zoom.fitPage")
      : `${zoomPercent}%`;

  return (
    <footer className="statusbar">
      <div className="status-left">
        <IconButton
          icon={ChevronLeft}
          label={t("menu.go.previousPage")}
          shortcut={shortcutFor("go.previousPage", os)}
          disabled={!info || currentPage <= 0}
          size={16}
          onClick={() => goToPage(Math.max(0, currentPage - 1))}
        />
        <form
          className="page-jump"
          onSubmit={(e) => {
            e.preventDefault();
            const page = pageForEntry(pageField, labels, total);
            if (page !== null) goToPage(page);
            else setPageField(displayLabel(labels, currentPage));
          }}
        >
          <input
            className="page-input mono text-sm"
            data-labels={labels ? "" : undefined}
            value={pageField}
            inputMode={labels ? "text" : "numeric"}
            aria-label={t("view.goToPage.placeholder")}
            disabled={!info}
            onChange={(e) => setPageField(labels ? e.target.value : e.target.value.replace(/[^\d]/g, ""))}
            onBlur={() => setPageField(displayLabel(labels, currentPage))}
          />
          <span className="text-sm dim">
            {labels ? `(${currentPage + 1} / ${total})` : `/ ${total || "—"}`}
          </span>
        </form>
        <IconButton
          icon={ChevronRight}
          label={t("menu.go.nextPage")}
          shortcut={shortcutFor("go.nextPage", os)}
          disabled={!info || currentPage >= total - 1}
          size={16}
          onClick={() => goToPage(Math.min(total - 1, currentPage + 1))}
        />

        <span className="status-sep" />

        <div className="segmented small" role="group" aria-label={t("view.layout.continuous")}>
          {LAYOUTS.map((l) => (
            <button
              key={l.id}
              type="button"
              className="segment"
              data-active={layout === l.id || undefined}
              disabled={!info}
              onClick={() => setLayout(l.id)}
            >
              {t(l.labelKey)}
            </button>
          ))}
        </div>

        <span className="status-sep" />

        <IconButton
          icon={RotateCcw}
          label={t("view.rotateLeft")}
          shortcut={shortcutFor("view.rotateLeft", os)}
          disabled={!info}
          size={16}
          onClick={() => rotate(-90)}
        />
        <IconButton
          icon={RotateCw}
          label={t("view.rotateRight")}
          shortcut={shortcutFor("view.rotateRight", os)}
          disabled={!info}
          size={16}
          onClick={() => rotate(90)}
        />
        {/* 야간 모드 (P1-10): one click steps 끄기 → 어둡게 → 세피아, like ⌃⌘N and 보기 ▸ 야간 모드. */}
        <IconButton
          icon={Moon}
          label={`${t("view.night.label")}: ${t(`view.night.${night}`)}`}
          shortcut={shortcutFor("view.night", os)}
          active={night !== "off"}
          disabled={!info}
          size={16}
          onClick={cycleNight}
        />
        {/* 분할 보기 (P2): the toggle, then — while split — 좌우 ⇄ 위아래 and 동기화 스크롤 */}
        <IconButton
          icon={Columns2}
          label={t("view.split.label")}
          shortcut={shortcutFor("view.split", os)}
          active={!!split}
          disabled={!info}
          size={16}
          onClick={toggleSplit}
        />
        {split && (
          <>
            <IconButton
              icon={split.orientation === "side" ? Rows2 : Columns2}
              label={t(split.orientation === "side" ? "view.split.stacked" : "view.split.side")}
              size={16}
              onClick={() => setSplitOrientation(split.orientation === "side" ? "stacked" : "side")}
            />
            <IconButton
              icon={Link2}
              label={t("view.split.sync")}
              active={split.sync}
              size={16}
              onClick={() => {
                if (!split.sync) resyncPanes();
                setSyncScroll(!split.sync);
              }}
            />
          </>
        )}
      </div>

      <div className="status-centre">
        <IconButton icon={ZoomOut} label={t("view.zoom.out")} disabled={!info} size={16} onClick={zoomOut} />
        <input
          className="zoom-slider"
          type="range"
          min={25}
          max={400}
          step={1}
          value={zoomPercent}
          disabled={!info}
          aria-label={t("view.zoom.in")}
          onChange={(e) => setZoom(Number(e.target.value))}
        />
        <IconButton icon={ZoomIn} label={t("view.zoom.in")} disabled={!info} size={16} onClick={zoomIn} />
        <select
          className="zoom-combo text-sm mono"
          value={zoomMode === "custom" ? String(zoomPercent) : zoomMode}
          disabled={!info}
          aria-label={zoomLabel}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "fit-width" || v === "fit-page") setZoomMode(v);
            else if (v === "actual") setZoomMode("actual", 100);
            else setZoom(Number(v));
          }}
        >
          {ZOOM_STEPS.map((z) => (
            <option key={z} value={String(z)}>{`${z}%`}</option>
          ))}
          {/* A custom zoom off the preset list (slider, pinch, a restored recent) needs its own
              option, or the <select> falls back to showing the first one ("25%"). */}
          {zoomMode === "custom" && !(ZOOM_STEPS as readonly number[]).includes(zoomPercent) && (
            <option value={String(zoomPercent)} hidden>{`${zoomPercent}%`}</option>
          )}
          <option value="fit-page">{t("view.zoom.fitPage")}</option>
          <option value="fit-width">{t("view.zoom.fitWidth")}</option>
          <option value="actual">{t("view.zoom.actual")}</option>
        </select>
      </div>

      <div className="status-right">
        {/* 읽어 주기 (P2): pressed while the system voice speaks; a click stops it */}
        {speaking && (
          <IconButton
            icon={AudioLines}
            label={`${t("tts.speaking")} — ${t("tts.stop")}`}
            active
            size={16}
            onClick={() => void import("../tts/speak").then((m) => m.stopSpeaking())}
          />
        )}
        {job && job.state === "running" && (
          <div className="job-slot" role="status">
            <span className="text-sm">{t(job.labelKey)}</span>
            <progress className="job-bar" value={job.done} max={Math.max(1, job.total)} />
            <span className="text-xs dim mono">
              {job.total ? `${job.done}/${job.total}` : ""}
            </span>
            <IconButton icon={X} label={t("common.cancel")} size={14} onClick={() => void cancelJob(job.id)} />
          </div>
        )}
        {info && (
          <span className="text-sm dim save-state" data-dirty={info.dirty || undefined}>
            {info.dirty ? t("status.unsaved") : t("status.saved")}
          </span>
        )}
      </div>
    </footer>
  );
}
