/**
 * 워터마크 / 머리글·바닥글 (P1-4). One dialog, three roles: the role only picks the defaults
 * (anchor, rotation, size, opacity) and the undo label; the engine lays the stamp out from the
 * anchor + margin. The preview is a CSS approximation over the first page of the range (its
 * thumbnail, in the page's visual size: crop box, `/Rotate` applied — the space the engine lays
 * the stamp out in). An image stamp shows the picked file itself (v0.3 T1: `image_preview` → a
 * `blob:` URL — no asset protocol, no fs scope) at its real aspect; a placeholder box until then.
 * v0.3 T4: 뒤에 배치 puts the stamp under the page content, and 배경색 fills the page behind it.
 *
 * 워터마크 제거 (Stage 8): `remove_stamps` over the same page range, for the current role or every
 * SeePDF stamp (모두 제거 — the only way to reach stamps written before roles were recorded).
 * Lazy-loaded from `DialogHost`.
 */
import { useMemo, useRef, useState } from "react";
import { Image as ImageIcon } from "lucide-react";
import * as api from "../ipc/api";
import { thumbUrl } from "../ipc/protocol";
import { devicePixelRatio } from "../viewer/geometry";
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
  ANCHORS, BATES_MAX_AFFIX, BATES_MAX_DIGITS, STAMP_TOKENS, anchorStyle, batesLabel, batesPreset, buildStampSpec,
  expandTokens, imageStampHeight, initialStampForm, insertToken, isoDate, removeStampsArgs, stemOf, switchRole,
  usesBates, validateStamp, visualPageSize, type StampForm, type StampToken,
} from "./stamp";

const ROLES: StampRole[] = ["watermark", "header", "footer"];
/** v0.3 T4: 배경색 choices — pale paper tones. */
const BACKGROUND_COLORS: [number, number, number][] = [
  [255, 248, 220], [255, 243, 205], [232, 244, 253], [234, 248, 234], [253, 236, 240], [240, 240, 240],
];
/** preview page width in CSS px */
const PREVIEW_W = 168;

/**
 * A picked image as the preview needs it (v0.3 T1): its own pixel size and a `blob:` URL of a
 * small PNG from `image_preview` (`url` is `null` where the platform has no object URLs).
 * Replaceable in tests.
 */
export const stampImageSource = {
  async preview(path: string): Promise<{ url: string | null; w: number; h: number }> {
    const p = await api.imagePreview({ path, maxPx: 512 });
    return { url: api.previewUrl(p), w: p.width, h: p.height };
  },
};

export default function StampDialog({ onClose, role: initialRole }: { onClose(): void; role?: StampRole }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const currentPage = useViewStore((s) => s.currentPage);
  const selected = usePagesStore((s) => s.selected);
  const watermarkText = t("stamp.defaultText");
  const [form, setForm] = useState<StampForm>(() => initialStampForm(initialRole ?? "watermark", watermarkText));
  const [range, setRange] = useState<RangeChoice>({ mode: "all", text: "" });
  const [busy, setBusy] = useState(false);
  /** the picked image as `image_preview` described it: its preview URL and its own size */
  const [image, setImage] = useState<{ path: string; src: string | null; w: number; h: number } | null>(null);
  const [imageFailed, setImageFailed] = useState<string | null>(null);
  const textRef = useRef<HTMLTextAreaElement>(null);

  const pageCount = info?.pageCount ?? 0;
  const pages = useMemo(
    () => resolveRange(range, { pageCount, currentPage, selected }),
    [range, pageCount, currentPage, selected],
  );
  if (!info) return null;

  const patch = (p: Partial<StampForm>) => setForm((f) => ({ ...f, ...p }));
  const error = validateStamp(form, pages);
  // the first page of the range, in the space the engine lays the stamp out in
  const previewPage = pages?.[0] ?? 0;
  const { widthPt: pageW, heightPt: pageH } = visualPageSize(info.pages[previewPage]);
  const scale = PREVIEW_W / pageW;
  const loaded = form.source === "image" && image && image.path === form.imagePath && imageFailed !== form.imagePath ? image : null;
  const imageSrc = loaded?.src ?? null;

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
    if (!picked?.length) return;
    const path = picked[0];
    patch({ imagePath: path });
    try {
      const p = await stampImageSource.preview(path);
      setImage({ path, src: p.url, w: p.w, h: p.h });
    } catch {
      setImageFailed(path);
    }
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

  /** 워터마크 제거: the chosen role (or every role) on the chosen range; the dialog stays open. */
  const remove = async (scope: "role" | "all") => {
    if (!pages?.length) return;
    setBusy(true);
    try {
      const args = removeStampsArgs(scope, form.role, pages, range.mode === "all");
      const result = await api.removeStamps({ docId: info.docId, ...args });
      useDocStore.getState().adopt(result.info);
      if (result.removed === 0) toast("stamp.remove.none", undefined, { tone: "info" });
      else
        toast("stamp.remove.done", { count: result.removed }, {
          tone: "success",
          actions: [{ labelKey: "common.undo", onSelect: () => void import("../annot/sync").then((m) => m.undoWithAnnots()) }],
        });
    } catch (e) {
      toast("stamp.remove.failed", undefined, { tone: "danger", detail: message(e) });
    } finally {
      setBusy(false);
    }
  };

  const previewText = expandTokens(form.text, {
    page: previewPage + 1,
    total: info.pageCount,
    date: isoDate(),
    filename: stemOf(info.name),
    bates: batesLabel(form, 0),
  });
  const bates = form.source === "text" && usesBates(form.text);
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
            <div className="inline-row">
              <button type="button" className="btn quiet" onClick={() => setForm((f) => batesPreset(f))}>
                {t("stamp.bates.preset")}
              </button>
            </div>
          </Row>

          <Row labelKey="stamp.source">
            <div className="segmented small" role="radiogroup" aria-label={t("stamp.source")}>
              {(["text", "image", "background"] as const).map((s) => (
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

          {form.source === "background" ? (
            <Row labelKey="stamp.background.color">
              {/* v0.3 T4: pale paper tones first (they read better behind text), then the palette + custom */}
              <div className="swatches" role="radiogroup" aria-label={t("stamp.background.paper")}>
                {BACKGROUND_COLORS.map((c) => (
                  <button
                    key={c.join()}
                    type="button"
                    role="radio"
                    aria-checked={c.join() === form.backgroundColor.join()}
                    aria-label={rgbCss(c)}
                    className="swatch"
                    data-selected={c.join() === form.backgroundColor.join() || undefined}
                    style={{ background: rgbCss(c) }}
                    onClick={() => patch({ backgroundColor: c })}
                  />
                ))}
              </div>
              <Swatches value={form.backgroundColor} label={t("stamp.background.color")} onPick={(backgroundColor) => patch({ backgroundColor })} />
              <p className="dlg-hint text-xs">{t("stamp.background.hint")}</p>
            </Row>
          ) : form.source === "text" ? (
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
              {bates && (
                <Row labelKey="stamp.bates.section">
                  <div className="stamp-bates">
                    <label className="inline-row text-sm">
                      <span className="dim">{t("stamp.bates.start")}</span>
                      <input
                        className="field num"
                        type="number"
                        min={0}
                        step={1}
                        aria-label={t("stamp.bates.start")}
                        value={form.batesStart}
                        onChange={(e) => patch({ batesStart: num(e.target.value, form.batesStart) })}
                      />
                    </label>
                    <label className="inline-row text-sm">
                      <span className="dim">{t("stamp.bates.digits")}</span>
                      <input
                        className="field num"
                        type="number"
                        min={1}
                        max={BATES_MAX_DIGITS}
                        step={1}
                        aria-label={t("stamp.bates.digits")}
                        value={form.batesDigits}
                        onChange={(e) => patch({ batesDigits: num(e.target.value, form.batesDigits) })}
                      />
                    </label>
                    <label className="inline-row text-sm">
                      <span className="dim">{t("stamp.bates.prefix")}</span>
                      <input
                        className="field"
                        maxLength={BATES_MAX_AFFIX}
                        aria-label={t("stamp.bates.prefix")}
                        value={form.batesPrefix}
                        onChange={(e) => patch({ batesPrefix: e.target.value })}
                      />
                    </label>
                    <label className="inline-row text-sm">
                      <span className="dim">{t("stamp.bates.suffix")}</span>
                      <input
                        className="field"
                        maxLength={BATES_MAX_AFFIX}
                        aria-label={t("stamp.bates.suffix")}
                        value={form.batesSuffix}
                        onChange={(e) => patch({ batesSuffix: e.target.value })}
                      />
                    </label>
                  </div>
                  <p className="dlg-hint text-xs mono" data-testid="bates-sample">
                    {t("stamp.bates.sample", {
                      first: batesLabel(form, 0),
                      last: batesLabel(form, Math.max(0, (pages?.length ?? 1) - 1)),
                    })}
                  </p>
                  <p className="dlg-hint text-xs">{t("stamp.bates.hint")}</p>
                </Row>
              )}
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
                {error === "noImage" && <p className="dlg-hint danger text-xs">{t("stamp.error.noImage")}</p>}
                {form.imagePath && imageFailed === form.imagePath && <p className="dlg-hint danger text-xs">{t("stamp.image.unreadable")}</p>}
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

          {form.source !== "background" && (
            <label className="inline-row text-sm">
              <input type="checkbox" checked={form.behind} onChange={(e) => patch({ behind: e.currentTarget.checked })} />
              {t("stamp.behind")}
            </label>
          )}

          {form.role === "watermark" && form.source !== "background" && (
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

          <Row labelKey="stamp.remove.section">
            <div className="inline-row">
              <button
                type="button"
                className="btn"
                data-danger
                disabled={busy || !pages?.length}
                onClick={() => void remove("role")}
              >
                {t(`stamp.remove.role.${form.role}`)}
              </button>
              <button
                type="button"
                className="btn quiet"
                data-danger
                disabled={busy || !pages?.length}
                onClick={() => void remove("all")}
              >
                {t("stamp.remove.all")}
              </button>
            </div>
            <p className="dlg-hint text-xs">{t("stamp.remove.hint")}</p>
          </Row>
        </div>

        <figure className="stamp-preview" aria-label={t("stamp.preview")}>
          <div
            className="stamp-page"
            data-testid="stamp-preview-page"
            style={{ width: PREVIEW_W, height: Math.round(pageH * scale) }}
          >
            <img
              className="stamp-thumb"
              src={thumbUrl({
                doc: info.docId,
                gen: info.docGeneration,
                page: previewPage,
                w: Math.round(PREVIEW_W * devicePixelRatio()),
              })}
              alt=""
              draggable={false}
            />
            {form.source === "background" && (
              <div
                className="stamp-background"
                data-testid="stamp-preview-background"
                style={{ background: rgbCss(form.backgroundColor), opacity: form.opacityPct / 100 }}
              />
            )}
            <div
              className="stamp-mark"
              data-testid="stamp-preview-mark"
              data-behind={form.behind || undefined}
              hidden={form.source === "background"}
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
                  data-loaded={loaded ? true : undefined}
                  style={{
                    width: Math.max(8, form.imageWidthPt * scale),
                    height: Math.max(6, imageStampHeight(form.imageWidthPt, loaded?.w, loaded?.h) * scale),
                  }}
                >
                  {imageSrc && form.imagePath ? (
                    <img
                      key={imageSrc}
                      src={imageSrc}
                      alt=""
                      data-testid="stamp-preview-image"
                      draggable={false}
                      onError={() => setImageFailed(form.imagePath)}
                    />
                  ) : null}
                  {!imageSrc && <ImageIcon size={12} strokeWidth={1.75} aria-hidden />}
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
