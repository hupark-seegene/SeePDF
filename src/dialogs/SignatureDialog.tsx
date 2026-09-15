/**
 * 서명 만들기 (UI_SPEC §6, STAGE1D_NOTES §7.5).
 *
 * The sheet that was missing: arming 서명 used to open a file picker, so a user with no
 * signature image could not sign at all. Draw with a pointer in the canvas below, confirm, and
 * the strokes become the **pending signature** of `src/tools/stamp.ts`; the next click on a
 * page commits them as an `AnnotSpec` of kind `signature`, which the engine writes as a real
 * `/Ink` annotation with `/Subj "SeePDF:Signature"` (`engine::annot::create::ink`). That path
 * already existed and is covered by `cargo test annot_signature_subj_roundtrip`; only this
 * sheet was missing.
 *
 * Strokes are kept in **unit space** (0…1 of the drawn bounding box, y-down) so the same
 * signature can be placed at any size on any page; `stamp.ts` maps them into the placement
 * rectangle in PDF points.
 *
 * 이미지 선택… stays available for a scanned signature — it hands over to the existing image
 * picker, so this sheet strictly adds a path rather than replacing one.
 */
import { useCallback, useRef, useState } from "react";
import { useT } from "../i18n/useT";
import { Dialog } from "./Dialog";
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

export interface SignatureDialogProps {
  onClose(): void;
  onDrawn(signature: DrawnSignature): void;
  /** 이미지 선택… — hand over to the existing file picker. */
  onChooseImage(): void;
}

export function SignatureDialog({ onClose, onDrawn, onChooseImage }: SignatureDialogProps) {
  const t = useT();
  const canvas = useRef<HTMLCanvasElement | null>(null);
  const strokes = useRef<number[][]>([]);
  const drawing = useRef<number[] | null>(null);
  const [empty, setEmpty] = useState(true);

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
      ((e.clientX - box.left) / box.width) * CANVAS_W,
      ((e.clientY - box.top) / box.height) * CANVAS_H,
    ];
  };

  const down = (e: React.PointerEvent<HTMLCanvasElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId);
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
    const ctx = context();
    ctx?.clearRect(0, 0, CANVAS_W, CANVAS_H);
    setEmpty(true);
  }, []);

  const confirm = () => {
    const normalised = normaliseStrokes(strokes.current);
    if (!normalised) return;
    onDrawn(normalised);
    onClose();
  };

  return (
    <Dialog
      titleKey="sign.create"
      size="lg"
      onClose={onClose}
      primary={{ labelKey: "common.ok", onSelect: confirm, disabled: empty }}
      footerExtra={
        <>
          <button type="button" className="btn quiet" onClick={clear} disabled={empty}>
            {t("sign.clear")}
          </button>
          <button type="button" className="btn quiet" onClick={onChooseImage}>
            {t("sign.chooseImage")}
          </button>
        </>
      }
    >
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
    </Dialog>
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
