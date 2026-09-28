/**
 * The per-context contents of the properties panel (UI_SPEC §7).
 *
 * Which sections appear is decided by *what is selected*, falling back to *what tool is armed* —
 * with nothing selected the panel edits the defaults the next annotation will be created with, and
 * with a selection it edits the selection. `patch.ts` owns that mapping and is unit-tested; this
 * file is the markup around it.
 */
import { useMemo } from "react";
import type { Annot, AnnotKind } from "../../ipc/types";
import { formatRelativeDay } from "../../i18n";
import { useT } from "../../i18n/useT";
import { useAppStore } from "../../store/appStore";
import { MARKUP_KINDS, useAnnotStore } from "../../store/annotStore";
import { commitField, useFormStore } from "../../forms/formStore";
import { isStyledTool, toolOfKind } from "../../store/toolStyles";
import { reopenStampPicker } from "../../tools/stamp";
import { makeApply } from "./apply";
import { EditPanel } from "./EditPanel";
import { LinkPanel } from "../../edit/LinkPanel";
import { Swatches } from "../Swatches";
import type { PropertyId } from "./patch";

const WIDTHS = [1, 2, 4, 8, 12];
const FONT_SIZES = [8, 10, 12, 14, 18, 24, 36];

/** The selected annotations, resolved through the store (ghosts included). */
function useSelection(): Annot[] {
  const selected = useAnnotStore((s) => s.selected);
  const nonce = useAnnotStore((s) => s.listNonce);
  const ghosts = useAnnotStore((s) => s.ghosts);
  return useMemo(() => {
    const find = useAnnotStore.getState().find;
    return selected.map((id) => find(id)?.annot).filter((a): a is Annot => !!a);
    // `listNonce` and `ghosts` are the invalidation signal; `find` reads the live store.
  }, [selected, nonce, ghosts]);
}

function useApply() {
  const setStyle = useAnnotStore((s) => s.setStyle);
  const selection = useSelection();
  return makeApply(selection, setStyle);
}

function Slider({ id, label, value, min, max, onChange }: {
  id: PropertyId;
  label: string;
  value: number;
  min: number;
  max: number;
  onChange(id: PropertyId, v: number): void;
}) {
  return (
    <>
      <h3 className="field-label text-xs">{label}</h3>
      <input
        className="slider"
        type="range"
        min={min}
        max={max}
        value={value}
        aria-label={label}
        onChange={(e) => onChange(id, Number(e.target.value))}
      />
    </>
  );
}

function Chips({ values, value, onPick }: { values: number[]; value: number; onPick(v: number): void }) {
  return (
    <div className="chip-row">
      {values.map((v) => (
        <button key={v} type="button" className="chip mono" data-active={value === v || undefined} onClick={() => onPick(v)}>
          {v}
        </button>
      ))}
    </div>
  );
}

function AnnotMeta({ annot }: { annot: Annot }) {
  const t = useT();
  return (
    <dl className="inspector-meta">
      <dt>{t("prop.author")}</dt>
      <dd>{annot.author ?? "—"}</dd>
      <dt>{t("prop.created")}</dt>
      <dd>{annot.created ? formatRelativeDay(annot.created) : "—"}</dd>
      <dt>{t("prop.modified")}</dt>
      <dd>{annot.modified ? formatRelativeDay(annot.modified) : "—"}</dd>
    </dl>
  );
}

function FormPanel() {
  const t = useT();
  const focused = useFormStore((s) => s.focused);
  const fields = useFormStore((s) => s.fields);
  const field = useMemo(() => fields.find((f) => `${f.page}:${f.index}` === focused), [fields, focused]);
  if (!field) return <p className="empty">{t("form.fieldCount", { count: fields.length })}</p>;
  return (
    <section className="field-group">
      <dl className="inspector-meta">
        <dt>{t("form.fieldName")}</dt>
        <dd>{field.name}</dd>
        <dt>{t("form.fieldType")}</dt>
        <dd>{field.type}</dd>
        {field.required && (
          <>
            <dt>{t("form.required")}</dt>
            <dd>✓</dd>
          </>
        )}
      </dl>
      <button
        type="button"
        className="btn quiet"
        onClick={() => void commitField(field.page, field.index, field.type === "checkbox" || field.type === "radio" ? { checked: false } : { text: "" })}
      >
        {t("form.clearField")}
      </button>
    </section>
  );
}

export function InspectorBody() {
  const t = useT();
  const mode = useAppStore((s) => s.mode);
  const tool = useAppStore((s) => s.tool);
  const style = useAnnotStore((s) => s.style);
  const styleTool = useAnnotStore((s) => s.styleTool);
  const overrides = useAnnotStore((s) => s.toolDefaults);
  const setToolDefault = useAnnotStore((s) => s.setToolDefault);
  const resetToolDefault = useAnnotStore((s) => s.resetToolDefault);
  const selection = useSelection();
  const apply = useApply();

  if (mode === "form") return <FormPanel />;
  if (mode === "edit") return <EditPanel />;
  if (mode !== "annotate") return <p className="empty">{t("prop.empty")}</p>;

  const one = selection.length === 1 ? selection[0] : null;
  // P2: a link has no style — its section is the target
  if (one?.kind === "link") return <LinkPanel link={{ page: one.page, id: one.id }} />;
  const kind = one?.kind ?? kindOfTool(tool);
  const colour = one?.color ?? style.color;
  const opacity = one?.opacity ?? style.opacity;
  const width = one?.borderWidth ?? style.width;
  const fill = one ? one.fillColor : style.fillColor;
  const isShape = kind === "square" || kind === "circle";
  const isLineLike = kind === "line" || kind === "arrow";
  const isText = kind === "textbox" || kind === "stamp";
  // 굵기 has no meaning for a markup run or a sticky note — UI_SPEC §7 lists it only for
  // 펜 / 도형 / 선, and `AnnotPatch.borderWidth` on a Highlight is silently ignored by the engine.
  const isMarkup = MARKUP_KINDS.includes(kind as AnnotKind) || kind === "note";
  const showThickness = !isText && !isMarkup;
  // 도구별 기본 스타일 (P1-12): which tool's default the buttons below talk about — the selected
  // annotation's tool, or (nothing selected) the armed tool / the one whose style is showing.
  const defaultTool = selection.length > 0 ? toolOfKind(one?.kind) : isStyledTool(tool) ? tool : styleTool;
  const hasOverride = !!defaultTool && !!overrides[defaultTool];
  const placesImage = (tool === "stamp" || tool === "signature") && selection.length === 0;

  return (
    <>
      {selection.length > 1 && <p className="field-label text-xs">{t("sidebar.annotations.count", { count: selection.length })}</p>}

      <section className="field-group">
        <h3 className="field-label text-xs">{isShape || isLineLike ? t("prop.strokeColor") : t("prop.color")}</h3>
        <Swatches value={colour} label={t("prop.color")} onPick={(c) => apply("color", c)} />

        {(isShape || isText) && (
          <>
            <h3 className="field-label text-xs">{t("prop.fillColor")}</h3>
            <Swatches value={fill} label={t("prop.fillColor")} onPick={(c) => apply("fillColor", c)} />
            <button type="button" className="chip" data-active={fill === null || undefined} onClick={() => apply("fillColor", null)}>
              {t("prop.noFill")}
            </button>
          </>
        )}

        <Slider id="opacity" label={t("prop.opacity")} value={Math.round(opacity * 100)} min={5} max={100} onChange={(id, v) => apply(id, v / 100)} />

        {showThickness && (
          <>
            <h3 className="field-label text-xs">{t("prop.thickness")}</h3>
            <Chips values={WIDTHS} value={width} onPick={(v) => apply("width", v)} />
          </>
        )}

        {isLineLike && (
          <div className="inspector-row">
            <label className="text-sm">
              <input type="checkbox" checked={style.heads[0]} onChange={(e) => apply("heads", [e.currentTarget.checked, style.heads[1]])} />
              {t("prop.arrowStart")}
            </label>
            <label className="text-sm">
              <input type="checkbox" checked={style.heads[1]} onChange={(e) => apply("heads", [style.heads[0], e.currentTarget.checked])} />
              {t("prop.arrowEnd")}
            </label>
          </div>
        )}

        {isText && (
          <>
            <h3 className="field-label text-xs">{t("prop.fontSize")}</h3>
            <Chips values={FONT_SIZES} value={one?.fontSize ?? style.fontSize} onPick={(v) => apply("fontSize", v)} />
            <h3 className="field-label text-xs">{t("prop.align")}</h3>
            <div className="chip-row">
              {(["left", "center", "right"] as const).map((a) => (
                <button key={a} type="button" className="chip" data-active={style.align === a || undefined} onClick={() => apply("align", a)}>
                  {t(`prop.align.${a}`)}
                </button>
              ))}
            </div>
          </>
        )}

        {tool === "eraser" && (
          <Slider id="eraserSize" label={t("prop.eraserSize")} value={style.eraserSize} min={4} max={48} onChange={(id, v) => apply(id, v)} />
        )}
      </section>

      {one && (
        <section className="field-group">
          <h3 className="field-label text-xs">{t("prop.note")}</h3>
          <textarea
            key={one.id}
            className="field inspector-note"
            defaultValue={one.kind === "textbox" ? (one.text ?? one.contents) : one.contents}
            placeholder={t("prop.note.placeholder")}
            onBlur={(e) => apply(one.kind === "textbox" ? "text" : "contents", e.currentTarget.value)}
          />
          <AnnotMeta annot={one} />
        </section>
      )}

      <section className="field-group">
        {placesImage && (
          <button type="button" className="btn quiet" onClick={() => reopenStampPicker(tool as "stamp" | "signature")}>
            {t(tool === "stamp" ? "prop.changeStamp" : "prop.changeSignature")}
          </button>
        )}
        {selection.length > 0 && defaultTool && (
          <button
            type="button"
            className="btn quiet"
            onClick={() =>
              setToolDefault(defaultTool, {
                color: colour,
                opacity,
                ...(showThickness ? { width } : {}),
                ...(isShape || isText ? { fillColor: fill } : {}),
                ...(one?.fontSize ? { fontSize: one.fontSize } : {}),
              })
            }
          >
            {t("prop.setDefault")}
          </button>
        )}
        {selection.length === 0 && hasOverride && defaultTool && (
          <button type="button" className="btn quiet" onClick={() => resetToolDefault(defaultTool)}>
            {t("prop.resetDefault")}
          </button>
        )}
        {selection.length > 0 && (
          <button
            type="button"
            className="btn quiet"
            onClick={() => {
              void import("../../annot/actions").then((m) => {
                for (const a of selection) void m.deleteAnnotations(a.page, [a.id]);
              });
              useAnnotStore.getState().select([]);
            }}
          >
            {t("common.delete")}
          </button>
        )}
      </section>
    </>
  );
}

function kindOfTool(tool: string): Annot["kind"] | undefined {
  switch (tool) {
    case "highlight":
    case "underline":
    case "strikeout":
    case "squiggly":
    case "note":
    case "line":
    case "arrow":
    case "textbox":
    case "stamp":
    case "signature":
      return tool as Annot["kind"];
    case "rectangle":
      return "square";
    case "ellipse":
      return "circle";
    case "pen":
      return "ink";
    default:
      return undefined;
  }
}

export default InspectorBody;
