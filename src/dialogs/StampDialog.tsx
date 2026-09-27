/**
 * 워터마크 / 머리글·바닥글 (P1-4). One dialog, three roles: the role only picks the defaults
 * (anchor, rotation, size, opacity) and the undo label; the engine lays the stamp out from the
 * anchor + margin. The preview is a CSS approximation on a page-shaped box — no rendering.
 * Lazy-loaded from `DialogHost`.
 */
import { useMemo, useRef, useState } from "react";
import { Image as ImageIcon } from "lucide-react";
import * as api from "../ipc/api";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";
import { usePagesStore } from "../store/pagesStore";
import { toast } from "../app/toastStore";
import { Swatches, rgbCss } from "../app/Swatches";
import type { StampRole } from "../ipc/types";
import { Dialog, Row } from "./Dialog";
import { RangePicker } from "./RangePicker";
import { resolveRange, type RangeChoice } from "./pageRange";
import { message } from "./flows";
import {
  ANCHORS, STAMP_TOKENS, anchorStyle, buildStampSpec, expandTokens, initialStampForm, insertToken, isoDate, stemOf,
  switchRole, validateStamp, type StampForm, type StampToken,
} from "./stamp";

const ROLES: StampRole[] = ["watermark", "header", "footer"];
/** preview page width in CSS px */
const PREVIEW_W = 168;

export default function StampDialog({ onClose, role: initialRole }: { onClose(): void; role?: StampRole }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const selected = usePagesStore((s) => s.selected);
  const watermarkText = t("stamp.defaultText");
  const [form, setForm] = useState<StampForm>(() => initialStampForm(initialRole ?? "watermark", watermarkText));
  const [range, setRange] = useState<RangeChoice>({ mode: "all", text: "" });
  const [busy, setBusy] = useState(false);
  const textRef = useRef<HTMLTextAreaElement>(null);

  const pageCount = info?.pageCount ?? 0;
  const pages = useMemo(
    () => resolveRange(range, { pageCount, currentPage, selected }),
    [range, pageCount, currentPage, selected],
  );
  if (!info) return null;

  const patch = (p: Partial<StampForm>) => setForm((f) => ({ ...f, ...p }));
  const error = validateStamp(form, pages);
  const first = info.pages[0];
  const pageW = first?.widthPt || 612;
  const pageH = first?.heightPt || 792;
  const scale = PREVIEW_W / pageW;

  const addToken = (token: StampToken) => {
    const el = textRef.current;
    const next = insertToken(form.text, token, el?.selectionStart ?? form.text.length, el?.selectionEnd ?? undefined);
    patch({ text: next.text });
    requestAnimationFrame(() => {
      el?.focus();
      el?.setSelectionRange(next.caret, next.caret);
    });
  };

  const chooseImage = async () => {
    const picked = await api
      .openFileDialog({ multiple: false, filters: [{ name: "PNG / JPEG", extensions: ["png", "jpg", "jpeg"] }] })
      .catch(() => null);
    if (picked?.length) patch({ imagePath: picked[0] });
  };

  const apply = async () => {
    if (error || !pages) return;
    setBusy(true);
    try {
      const spec = buildStampSpec(form, pages, range.mode === "all");
      const result = await api.addStamp({ docId: info.docId, spec });
      useDocStore.getState().adopt(result.info);
      toast(
        form.role === "watermark" ? "stamp.done.watermark" : "stamp.done.headerFooter",
        { count: result.pagesStamped },
        {
          tone: "success",
          actions: [{ labelKey: "common.undo", onSelect: () => void import("../annot/sync").then((m) => m.undoWithAnnots()) }],
        },
      );
      onClose();
    } catch (e) {
      toast("stamp.failed", undefined, { tone: "danger", detail: message(e) });
      setBusy(false);
    }
  };

  const previewText = expandTokens(form.text, {
    page: 1,
    total: info.pageCount,
    date: isoDate(),
    filename: stemOf(info.name),
  });
  const pos = anchorStyle(form.anchor, form.marginPt, pageW, pageH);
  const num = (v: string, fallback: number) => {
    const n = Number(v);
    return Number.isFinite(n) ? n : fallback;
  };

  return (
    <Dialog
      titleKey="stamp.title"
      size="xl"
      onClose={onClose}
      primary={{ labelKey: "common.apply", onSelect: () => void apply(), disabled: busy || error !== null }}
    >
      <div className="stamp-grid">
        <div className="stamp-options">
          <Row labelKey="stamp.role">
            <div className="segmented" role="radiogroup" aria-label={t("stamp.role")}>
              {ROLES.map((r) => (
                <button
                  key={r}
                  type="button"
                  role="radio"
                  aria-checked={form.role === r}
                  className="segment"
                  data-active={form.role === r || undefined}
                  onClick={() => setForm((f) => switchRole(f, r, watermarkText))}
                >
                  {t(`stamp.role.${r}`)}
                </button>
              ))}
            </div>
          </Row>

          <Row labelKey="stamp.source">
            <div className="segmented small" role="radiogroup" aria-label={t("stamp.source")}>
              {(["text", "image"] as const).map((s) => (
                <button
                  key={s}
                  type="button"
                  role="radio"
                  aria-checked={form.source === s}
                  className="segment"
                  data-active={form.source === s || undefined}
                  onClick={() => patch({ source: s })}
                >
                  {t(`stamp.source.${s}`)}
                </button>
              ))}
            </div>
          </Row>

          {form.source === "text" ? (
            <>
              <Row labelKey="stamp.text">
                <textarea
                  ref={textRef}
                  className="field area stamp-text"
                  rows={2}
                  aria-label={t("stamp.text")}
                  value={form.text}
                  onChange={(e) => patch({ text: e.target.value })}
                />
                <div className="chip-row" role="group" aria-label={t("stamp.insert")}>
                  {STAMP_TOKENS.map((tok) => (
                    <button key={tok} type="button" className="chip" onClick={() => addToken(tok)}>
                      {t(`stamp.token.${tok}`)}
                    </button>
                  ))}
                </div>
                {error === "emptyText" && <p className="dlg-hint danger text-xs">{t("stamp.error.emptyText")}</p>}
              </Row>
              <Row labelKey="stamp.fontSize">
                <div className="inline-row">
                  <input
                    className="field num"
                    type="number"
                    min={4}
                    max={400}
                    aria-label={t("stamp.fontSize")}
                    value={form.fontSizePt}
                    onChange={(e) => patch({ fontSizePt: num(e.target.value, form.fontSizePt) })}
                  />
                  <span className="text-sm dim">pt</span>
                </div>
              </Row>
              <Row labelKey="prop.color">
                <Swatches value={form.color} label={t("prop.color")} onPick={(color) => patch({ color })} />
              </Row>
            </>
          ) : (
            <>
              <Row labelKey="stamp.source.image">
                <div className="inline-row">
                  <button type="button" className="btn" onClick={() => void chooseImage()}>
                    {t("stamp.image.choose")}
                  </button>
                  <span className="text-sm dim stamp-file">{form.imagePath ? baseName(form.imagePath) : t("stamp.image.none")}</span>
                </div>
              </Row>
              <Row labelKey="stamp.image.width">
                <div className="inline-row">
                  <input
                    className="field num"
                    type="number"
                    min={8}
                    max={2000}
                    aria-label={t("stamp.image.width")}
                    value={form.imageWidthPt}
                    onChange={(e) => patch({ imageWidthPt: num(e.target.value, form.imageWidthPt) })}
                  />
                  <span className="text-sm dim">pt</span>
                </div>
              </Row>
            </>
          )}

          <Row labelKey="prop.opacity">
            <div className="inline-row">
              <input
                className="slider"
                type="range"
                min={5}
                max={100}
                step={5}
                aria-label={t("prop.opacity")}
                value={form.opacityPct}
                onChange={(e) => patch({ opacityPct: Number(e.target.value) })}
              />
              <span className="text-sm mono">{form.opacityPct}%</span>
            </div>
          </Row>

          {form.role === "watermark" && (
            <Row labelKey="stamp.rotation">
              <div className="inline-row">
                <input
                  className="field num"
                  type="number"
                  min={-180}
                  max={180}
                  step={15}
                  aria-label={t("stamp.rotation")}
                  value={form.rotateDeg}
                  onChange={(e) => patch({ rotateDeg: num(e.target.value, form.rotateDeg) })}
                />
                <span className="text-sm dim">°</span>
              </div>
            </Row>
          )}

          <Row labelKey="prop.position">
            <div className="inline-row stamp-pos">
              <div className="anchor-grid" role="radiogroup" aria-label={t("prop.position")}>
                {ANCHORS.map((a) => (
                  <button
                    key={a}
                    type="button"
                    role="radio"
                    aria-checked={form.anchor === a}
                    aria-label={t(`stamp.anchor.${a}`)}
                    title={t(`stamp.anchor.${a}`)}
                    data-active={form.anchor === a || undefined}
                    onClick={() => patch({ anchor: a })}
                  />
                ))}
              </div>
              <label className="inline-row text-sm">
                <span className="dim">{t("stamp.margin")}</span>
                <input
                  className="field num"
                  type="number"
                  min={0}
                  max={500}
                  aria-label={t("stamp.margin")}
                  value={form.marginPt}
                  onChange={(e) => patch({ marginPt: num(e.target.value, form.marginPt) })}
                />
                <span className="dim">pt</span>
              </label>
            </div>
          </Row>

          <Row labelKey="pages.range">
            <RangePicker value={range} onChange={setRange} pageCount={pageCount} selectedCount={selected.length} />
          </Row>
        </div>

        <figure className="stamp-preview" aria-label={t("stamp.preview")}>
          <div className="stamp-page" style={{ width: PREVIEW_W, height: Math.round(pageH * scale) }}>
            <div
              className="stamp-mark"
              data-testid="stamp-preview-mark"
              style={{
                ...pos.style,
                transform: `${pos.translate} rotate(${-form.rotateDeg}deg)`,
                opacity: form.opacityPct / 100,
                // lines align like the engine's: to the anchored edge, centred otherwise
                textAlign: form.anchor[1] === "l" ? "left" : form.anchor[1] === "r" ? "right" : "center",
                color: rgbCss(form.color),
                fontSize: `${Math.max(3, form.fontSizePt * scale)}px`,
              }}
            >
              {form.source === "text" ? (
                previewText
              ) : (
                <span
                  className="stamp-image"
                  style={{ width: Math.max(8, form.imageWidthPt * scale), height: Math.max(6, form.imageWidthPt * scale * 0.6) }}
                >
                  <ImageIcon size={12} strokeWidth={1.75} aria-hidden />
                </span>
              )}
            </div>
          </div>
          <figcaption className="dlg-hint text-xs">{t("stamp.previewHint")}</figcaption>
        </figure>
      </div>
    </Dialog>
  );
}

function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}
