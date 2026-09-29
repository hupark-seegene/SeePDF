import { useEffect, useState } from "react";
import { AudioLines, Camera, ChevronLeft, ChevronRight, Columns2, Hand, Link2, Lock, Moon, MousePointer2, RotateCcw, RotateCw, Rows2, ShieldCheck, X, ZoomIn, ZoomOut } from "lucide-react";
import { IconButton } from "./IconButton";
import { Tooltip } from "./Tooltip";
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
import { stepPage } from "../viewer/stepPage";
import { toolController } from "../tools/ToolController";
import type { ToolId } from "../store/appStore";
import type { IconProps } from "./IconButton";
import type { ComponentType } from "react";

const LAYOUTS: { id: ViewLayout; labelKey: string; keyId: string }[] = [
  { id: "single", labelKey: "view.layout.single", keyId: "view.layout.single" },
  { id: "continuous", labelKey: "view.layout.continuous", keyId: "view.layout.continuous" },
  { id: "two", labelKey: "view.layout.twoPage", keyId: "view.layout.twoPage" },
  // v0.3 pkg6 (V5): 두 쪽 with the cover alone
  { id: "twoCover", labelKey: "view.layout.twoCoverShort", keyId: "view.layout.twoCover" },
];

/**
 * v0.3 pkg5 (V7): the 읽기 tools a mouse can latch — 선택 and 손 (drag pans a zoomed page; Esc returns
 * to 선택) — plus V1's (pkg6) 스냅샷, which is one-shot: the marquee's release hands back to 선택.
 */
const READ_TOOLS: { id: ToolId; labelKey: string; keyId: string; icon: ComponentType<IconProps> }[] = [
  { id: "select", labelKey: "tool.select", keyId: "tool.select", icon: MousePointer2 },
  { id: "hand", labelKey: "tool.hand", keyId: "view.handTool", icon: Hand },
  { id: "snapshot", labelKey: "tool.snapshot", keyId: "tool.snapshot", icon: Camera },
];

/** v0.3 pkg5 (H3): 진행률 N퍼센트 for a determinate bar, nothing for an indeterminate one. */
export function progressValueText(done: number, total: number, t: (key: string, p?: Record<string, string | number>) => string): string | undefined {
  if (!total) return undefined;
  return t("a11y.progress", { percent: Math.round((Math.min(done, total) / total) * 100) });
}

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
  const mode = useAppStore((s) => s.mode);
  const tool = useAppStore((s) => s.tool);
  const momentaryFrom = useAppStore((s) => s.momentaryFrom);

  // P2: with page labels the box shows the current page's label and accepts a label or a number
  const labels = info?.pageLabels;
  const [pageField, setPageField] = useState(displayLabel(labels, currentPage));
  useEffect(() => setPageField(displayLabel(labels, currentPage)), [currentPage, labels]);

  const total = info?.pageCount ?? 0;
  // 두 쪽 steps a whole spread (the current page is the spread's left page)
  const previousPage = stepPage(currentPage, -1, total, layout);
  const nextPage = stepPage(currentPage, 1, total, layout);
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
          disabled={!info || previousPage >= currentPage}
          size={16}
          onClick={() => goToPage(previousPage)}
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
          disabled={!info || nextPage <= currentPage}
          size={16}
          onClick={() => goToPage(nextPage)}
        />

        {/* v0.3 pkg5 (V7): 선택 / 손 in 읽기, latched (a click, not a hold); Esc returns to 선택 */}
        {mode === "read" && info && (
          <>
            <span className="status-sep" />
            <div className="segmented small status-tools" role="group" aria-label={t("menu.tools")}>
              {READ_TOOLS.map((rt) => {
                // a held Space shows 손 as the tool of the moment, but the latch is still 선택
                const latched = (momentaryFrom ?? tool) === rt.id;
                const Icon = rt.icon;
                return (
                  <Tooltip key={rt.id} label={t(rt.labelKey)} shortcut={shortcutFor(rt.keyId, os)}>
                    <button
                      type="button"
                      className="segment"
                      data-active={latched || undefined}
                      aria-pressed={latched}
                      aria-label={t(rt.labelKey)}
                      onClick={() => {
                        useAppStore.getState().setTool(rt.id);
                        toolController.arm(rt.id);
                      }}
                    >
                      <Icon size={14} strokeWidth={1.75} aria-hidden />
                    </button>
                  </Tooltip>
                );
              })}
            </div>
          </>
        )}

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
            <progress
              className="job-bar"
              value={job.done}
              max={Math.max(1, job.total)}
              aria-label={t(job.labelKey)}
              aria-valuetext={progressValueText(job.done, job.total, t)}
            />
            <span className="text-xs dim mono">
              {job.total ? `${job.done}/${job.total}` : ""}
            </span>
            {/* a save is one atomic rewrite: its job id is progress-only (`cancel_job` answers false) */}
            {job.kind !== "save" && (
              <IconButton icon={X} label={t("common.cancel")} size={14} onClick={() => void cancelJob(job.id)} />
            )}
          </div>
        )}
        {/* v0.3 pkg5 (H3): why editing may be refused — the PDF's own permissions forbid changes
            (읽기 전용), or it is password-protected (암호화됨) */}
        {info && !info.permissions.modify && (
          <span className="status-badge text-xs" data-badge="readOnly">
            <Lock size={12} strokeWidth={1.75} aria-hidden />
            {t("status.readOnly")}
          </span>
        )}
        {info?.encrypted && (
          <span className="status-badge text-xs" data-badge="encrypted">
            <ShieldCheck size={12} strokeWidth={1.75} aria-hidden />
            {t("status.encrypted")}
          </span>
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
