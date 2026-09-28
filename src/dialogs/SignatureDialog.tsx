/**
 * 서명 만들기 (UI_SPEC §6, §15.10; STAGE1D_NOTES §7.5; P1-9).
 *
 * Three ways to make a signature, one tab each:
 *
 * * **그리기** — draw with a pointer. Confirm hands unit-space strokes to `src/tools/stamp.ts`
 *   as the pending signature; the next click on a page commits `AnnotSpec { kind: "signature" }`,
 *   a real `/Ink` annotation with `/Subj "SeePDF:Signature"` (cargo `annot_signature_subj_roundtrip`).
 * * **입력** — type a name and pick one of three script-like styles (system fonts only, see
 *   `typedSignature.ts`). Confirm renders it to a transparent PNG, `write_signature_image` puts it
 *   under `$APPDATA/SeePDF/signatures/`, and the 서명 tool places it as `StampImage { path }` — the
 *   image-signature path that already existed, with the PNG's aspect kept.
 * * **이미지** — hands over to the existing file picker (a scanned signature).
 *
 * **저장된 서명** (보관함): with 이 서명 저장 ticked, confirm also appends the signature to
 * `Settings.signatures` (drawn: its strokes; typed: text + style, re-rendered on use), at most 10.
 * A click on a saved one places it straight away; × deletes it.
 *
 * Strokes are kept in **unit space** (0…1 of the drawn bounding box, y-down) so the same
 * signature can be placed at any size on any page; `stamp.ts` maps them into the placement
 * rectangle in PDF points.
 */
import { useCallback, useMemo, useRef, useState } from "react";
import { X } from "lucide-react";
import * as api from "../ipc/api";
import type { SavedSignature } from "../ipc/types";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { toast } from "../app/toastStore";
import { Dialog } from "./Dialog";
import {
  MAX_SAVED_SIGNATURES,
  addSignature,
  isFull,
  readSignatures,
  removeSignature,
  savedFromDrawn,
  savedFromTyped,
} from "./signatureLibrary";
import { DEFAULT_SIGNATURE_STYLE, MAX_TYPED_LENGTH, SIGNATURE_STYLES, renderTypedSignature, signatureStyle } from "./typedSignature";
import "./dialogs.css";

/** Canvas backing-store size. Points are normalised on confirm, so this is only resolution. */
const CANVAS_W = 600;
const CANVAS_H = 200;
const STROKE_PX = 3;

/** What the sheet hands back: unit-space strokes plus the shape they were drawn in. */
export interface DrawnSignature {
  /** `[x0,y0,x1,y1,…]` per stroke, 0…1 of the drawn bounding box, y-down. */
  paths: number[][];
  /** width ÷ height of that bounding box, so the placement rectangle keeps the shape. */
  aspect: number;
}

/** A typed signature, rendered and written: place it like a picked image, at this aspect. */
export interface ImageSignature {
  path: string;
  aspect: number;
}

export interface SignatureDialogProps {
  onClose(): void;
  onDrawn(signature: DrawnSignature): void;
  /** 입력 / a saved typed signature: the PNG is on disk, place it as an image */
  onImage?(signature: ImageSignature): void;
  /** 이미지 선택… — hand over to the existing file picker. */
  onChooseImage(): void;
}

type Tab = "draw" | "type" | "image";
const TABS: Tab[] = ["draw", "type", "image"];

/** Render a typed signature and write it where the engine can read it. */
async function typedToImage(text: string, style: string): Promise<ImageSignature> {
  const { bytes, aspect } = await renderTypedSignature(text, style);
  const path = await api.writeSignatureImage({ bytes });
  return { path, aspect };
}

export function SignatureDialog({ onClose, onDrawn, onImage, onChooseImage }: SignatureDialogProps) {
  const t = useT();
  const [tab, setTab] = useState<Tab>("draw");
  const canvas = useRef<HTMLCanvasElement | null>(null);
  const strokes = useRef<number[][]>([]);
  const drawing = useRef<number[] | null>(null);
  const [empty, setEmpty] = useState(true);
  const [typed, setTyped] = useState("");
  const [style, setStyle] = useState(DEFAULT_SIGNATURE_STYLE);
  const [save, setSave] = useState(false);
  const [busy, setBusy] = useState(false);

  const rawSaved = useAppStore((s) => s.settings?.signatures);
  const saved = useMemo(() => readSignatures(rawSaved), [rawSaved]);
  const full = isFull(saved);

  const persist = (next: SavedSignature[]) => useAppStore.getState().patchSettings({ signatures: next });

  const context = () => {
    const ctx = canvas.current?.getContext("2d");
    if (ctx) {
      ctx.lineWidth = STROKE_PX;
      ctx.lineCap = "round";
      ctx.lineJoin = "round";
      ctx.strokeStyle = "#111318";
    }
    return ctx;
  };

  /** Canvas-local coordinates in backing-store pixels. */
  const at = (e: React.PointerEvent<HTMLCanvasElement>): [number, number] => {
    const box = e.currentTarget.getBoundingClientRect();
    return [
      ((e.clientX - box.left) / (box.width || CANVAS_W)) * CANVAS_W,
      ((e.clientY - box.top) / (box.height || CANVAS_H)) * CANVAS_H,
    ];
  };

  const down = (e: React.PointerEvent<HTMLCanvasElement>) => {
    e.currentTarget.setPointerCapture?.(e.pointerId);
    const [x, y] = at(e);
    drawing.current = [x, y];
    const ctx = context();
    ctx?.beginPath();
    ctx?.moveTo(x, y);
  };

  const move = (e: React.PointerEvent<HTMLCanvasElement>) => {
    if (!drawing.current) return;
    const [x, y] = at(e);
    drawing.current.push(x, y);
    const ctx = context();
    ctx?.lineTo(x, y);
    ctx?.stroke();
  };

  const up = () => {
    const stroke = drawing.current;
    drawing.current = null;
    // A tap is two identical points; keep it — a dot is a legitimate part of a signature.
    if (stroke && stroke.length >= 2) {
      strokes.current.push(stroke);
      setEmpty(false);
    }
  };

  const clear = useCallback(() => {
    strokes.current = [];
    drawing.current = null;
    const ctx = canvas.current?.getContext("2d");
    ctx?.clearRect(0, 0, CANVAS_W, CANVAS_H);
    setEmpty(true);
  }, []);

  const remember = (entry: SavedSignature) => {
    if (!save) return;
    const next = addSignature(saved, entry);
    if (next && next !== saved) void persist(next);
  };

  const placeTyped = async (text: string, styleId: string): Promise<boolean> => {
    setBusy(true);
    try {
      const image = await typedToImage(text, styleId);
      onImage?.(image);
      return true;
    } catch {
      toast("sign.error.render", undefined, { tone: "danger" });
      return false;
    } finally {
      setBusy(false);
    }
  };

  const confirm = async () => {
    if (tab === "draw") {
      const normalised = normaliseStrokes(strokes.current);
      if (!normalised) return;
      remember(savedFromDrawn(normalised));
      onDrawn(normalised);
      onClose();
      return;
    }
    if (tab === "type") {
      const text = typed.trim();
      if (!text) return;
      if (await placeTyped(text, style)) {
        remember(savedFromTyped(text, style));
        onClose();
      }
    }
  };

  const pickSaved = async (entry: SavedSignature) => {
    if (busy) return;
    if (entry.kind === "drawn") {
      onDrawn({ paths: entry.paths, aspect: entry.aspect });
      onClose();
      return;
    }
    if (await placeTyped(entry.text, entry.style)) onClose();
  };

  const deleteSaved = (id: string) => void persist(removeSignature(saved, id));

  const canConfirm = !busy && (tab === "draw" ? !empty : tab === "type" ? typed.trim().length > 0 : false);
  const sample = typed.trim() || t("sign.typeSample");

  return (
    <Dialog
      titleKey="sign.create"
      size="lg"
      onClose={onClose}
      primary={{ labelKey: "common.ok", onSelect: () => void confirm(), disabled: !canConfirm }}
      footerExtra={
        tab === "draw" ? (
          <button type="button" className="btn quiet" onClick={clear} disabled={empty}>
            {t("sign.clear")}
          </button>
        ) : undefined
      }
    >
      <div className="segmented sign-tabs" role="tablist" aria-label={t("sign.create")}>
        {TABS.map((id) => (
          <button
            key={id}
            type="button"
            role="tab"
            aria-selected={tab === id}
            className="segment"
            data-active={tab === id || undefined}
            onClick={() => setTab(id)}
          >
            {t(`sign.${id}`)}
          </button>
        ))}
      </div>

      {/* The canvas stays mounted on every tab so a half-drawn signature survives a tab switch. */}
      <div className="sign-panel" role="tabpanel" hidden={tab !== "draw"}>
        <p className="text-sm dlg-hint">{t("sign.drawHint")}</p>
        <canvas
          className="sign-canvas"
          ref={canvas}
          width={CANVAS_W}
          height={CANVAS_H}
          aria-label={t("sign.drawHint")}
          onPointerDown={down}
          onPointerMove={move}
          onPointerUp={up}
          onPointerCancel={up}
        />
      </div>

      {tab === "type" && (
        <div className="sign-panel" role="tabpanel">
          <input
            className="field sign-name"
            type="text"
            value={typed}
            maxLength={MAX_TYPED_LENGTH}
            placeholder={t("sign.typePlaceholder")}
            aria-label={t("sign.typePlaceholder")}
            onChange={(e) => setTyped(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && canConfirm) void confirm();
            }}
          />
          <div className="sign-styles" role="radiogroup" aria-label={t("sign.style")}>
            {SIGNATURE_STYLES.map((s) => (
              <button
                key={s.id}
                type="button"
                role="radio"
                aria-checked={style === s.id}
                className="sign-style"
                data-active={style === s.id || undefined}
                onClick={() => setStyle(s.id)}
              >
                <span className="sign-style-sample" style={{ fontFamily: s.fontFamily, fontStyle: s.fontStyle }}>
                  {sample}
                </span>
                <span className="text-xs sign-style-name">{t(s.labelKey)}</span>
              </button>
            ))}
          </div>
        </div>
      )}

      {tab === "image" && (
        <div className="sign-panel sign-image" role="tabpanel">
          <p className="text-sm dlg-hint">{t("sign.imageHint")}</p>
          <button type="button" className="btn" onClick={onChooseImage}>
            {t("sign.chooseImage")}
          </button>
        </div>
      )}

      {tab !== "image" && (
        <label className="text-sm sign-save">
          <input type="checkbox" checked={save && !full} disabled={full} onChange={(e) => setSave(e.currentTarget.checked)} />
          {t("sign.save")}
          {full && <span className="dlg-hint">{t("sign.libraryFull", { max: MAX_SAVED_SIGNATURES })}</span>}
        </label>
      )}

      <section className="sign-library" aria-label={t("sign.saved")}>
        <h3 className="field-label text-xs">
          {t("sign.saved")} <span className="mono">{saved.length}/{MAX_SAVED_SIGNATURES}</span>
        </h3>
        {saved.length === 0 ? (
          <p className="text-sm dlg-hint">{t("sign.savedEmpty")}</p>
        ) : (
          <ul className="sign-saved-list">
            {saved.map((entry) => (
              <li key={entry.id} className="sign-saved">
                <button
                  type="button"
                  className="sign-saved-pick"
                  disabled={busy}
                  aria-label={t("sign.useSaved", { name: entry.kind === "typed" ? entry.text : t("sign.drawnName") })}
                  onClick={() => void pickSaved(entry)}
                >
                  <SavedPreview entry={entry} />
                </button>
                <button
                  type="button"
                  className="sign-saved-delete"
                  aria-label={t("sign.delete")}
                  title={t("sign.delete")}
                  onClick={() => deleteSaved(entry.id)}
                >
                  <X size={12} aria-hidden />
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </Dialog>
  );
}

/** A saved signature as a thumbnail: the strokes as SVG, or the text in its style. */
function SavedPreview({ entry }: { entry: SavedSignature }) {
  if (entry.kind === "typed") {
    const style = signatureStyle(entry.style);
    return (
      <span className="sign-saved-text" style={{ fontFamily: style.fontFamily, fontStyle: style.fontStyle }}>
        {entry.text}
      </span>
    );
  }
  const w = Math.max(0.2, entry.aspect) * 100;
  return (
    <svg className="sign-saved-svg" viewBox={`-4 -4 ${w + 8} 108`} aria-hidden>
      {entry.paths.map((stroke, i) => (
        <polyline
          key={i}
          points={stroke.map((v, j) => (j % 2 === 0 ? v * w : v * 100)).join(" ")}
          fill="none"
          stroke="currentColor"
          strokeWidth={4}
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      ))}
    </svg>
  );
}

/**
 * Canvas pixels → unit space (0…1 of the drawn bounding box, y-down) plus that box's aspect.
 *
 * Normalising against the **drawn** box rather than the canvas means the placed signature
 * fills its rectangle no matter where in the canvas the user signed, and carrying the aspect
 * means a tall autograph is not squashed into a wide default rectangle. A degenerate box (one
 * dot, a perfectly straight line) would divide by zero, so each axis falls back to 1 px.
 */
export function normaliseStrokes(strokes: number[][]): DrawnSignature | null {
  const points = strokes.flat();
  if (points.length < 2) return null;
  let minX = Infinity;
  let minY = Infinity;
  let maxX = -Infinity;
  let maxY = -Infinity;
  for (let i = 0; i < points.length; i += 2) {
    minX = Math.min(minX, points[i]);
    maxX = Math.max(maxX, points[i]);
    minY = Math.min(minY, points[i + 1]);
    maxY = Math.max(maxY, points[i + 1]);
  }
  const w = maxX - minX || 1;
  const h = maxY - minY || 1;
  const paths = strokes.map((stroke) => {
    const out = new Array<number>(stroke.length);
    for (let i = 0; i < stroke.length; i += 2) {
      out[i] = (stroke[i] - minX) / w;
      out[i + 1] = (stroke[i + 1] - minY) / h;
    }
    return out;
  });
  return { paths, aspect: w / h };
}

export default SignatureDialog;
