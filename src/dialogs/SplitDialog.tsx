/**
 * 문서 분할 (UI_SPEC §10): {{n}}쪽마다 / 페이지 범위로, plus the destination folder.
 * Progress goes to the status-bar job slot; the outputs land in a completion toast.
 */
import { useState } from "react";
import { FolderOpen } from "lucide-react";
import { useT } from "../i18n/useT";
import * as api from "../ipc/api";
import { useDocStore } from "../store/docStore";
import { useJobStore } from "../store/jobStore";
import { toast } from "../app/toastStore";
import { Dialog, Row } from "./Dialog";
import { dirName, message, revealAction } from "./flows";
import { parsePageRange } from "./pageRange";

export function SplitDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const [mode, setMode] = useState<"everyN" | "ranges">("everyN");
  const [everyN, setEveryN] = useState(1);
  const [ranges, setRanges] = useState("");
  const [outDir, setOutDir] = useState(() => (info?.path ? dirName(info.path) : ""));
  const [busy, setBusy] = useState(false);

  const rangeList = ranges
    .split(/\n/)
    .map((r) => r.trim())
    .filter(Boolean);
  const rangesValid = mode === "ranges" ? rangeList.length > 0 && rangeList.every((r) => parsePageRange(r, info?.pageCount ?? 1) !== null) : true;

  const pickDir = async () => {
    const picked = await api.openFileDialog({ directory: true, multiple: false });
    if (picked?.length) setOutDir(picked[0]);
  };

  const run = async () => {
    if (!info || !outDir) return;
    setBusy(true);
    const jobs = useJobStore.getState();
    try {
      await api.splitDocument(
        { docId: info.docId, mode: mode === "everyN" ? { everyN } : { ranges: rangeList }, outDir },
        (e) => {
          jobs.apply("split", "pages.split.title", e);
          if (e.type === "done") {
            toast("pages.split.done", { count: e.outputs?.length ?? 0 }, {
              tone: "success",
              actions: [revealAction(outDir)],
            });
          }
          if (e.type === "error") toast("error.generic", undefined, { tone: "danger", detail: e.error.message });
        },
      );
    } catch (e) {
      toast("error.generic", undefined, { tone: "danger", detail: message(e) });
    } finally {
      setBusy(false);
      onClose();
    }
  };

  return (
    <Dialog
      titleKey="pages.split.title"
      onClose={onClose}
      primary={{ labelKey: "common.apply", onSelect: () => void run(), disabled: busy || !outDir || !rangesValid }}
    >
      <div className="dlg-radio-group">
        <label className="dlg-radio text-base">
          <input type="radio" name="split" checked={mode === "everyN"} onChange={() => setMode("everyN")} />
          <span>{t("pages.split.everyN", { n: everyN })}</span>
        </label>
        {mode === "everyN" && (
          <input
            className="field num"
            type="number"
            min={1}
            max={Math.max(1, info?.pageCount ?? 1)}
            value={everyN}
            aria-label={t("pages.split.everyN", { n: everyN })}
            onChange={(e) => setEveryN(Math.max(1, Number(e.target.value) || 1))}
          />
        )}
        <label className="dlg-radio text-base">
          <input type="radio" name="split" checked={mode === "ranges"} onChange={() => setMode("ranges")} />
          <span>{t("pages.split.byRange")}</span>
        </label>
        {mode === "ranges" && (
          <textarea
            className="field area"
            rows={3}
            value={ranges}
            aria-label={t("pages.split.byRange")}
            placeholder={t("pages.range.placeholder")}
            onChange={(e) => setRanges(e.target.value)}
          />
        )}
      </div>

      <Row labelKey="pages.split.outDir">
        <div className="inline-row">
          <input className="field grow" value={outDir} readOnly aria-label={t("pages.split.outDir")} />
          <button type="button" className="btn" onClick={() => void pickDir()}>
            <FolderOpen size={16} strokeWidth={1.75} aria-hidden />
            {t("common.browse")}
          </button>
        </div>
      </Row>
    </Dialog>
  );
}
