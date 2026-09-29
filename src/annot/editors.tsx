/**
 * The two inline editors that float over a page: the 메모 popover (F-12) and the 텍스트 상자 editor
 * (F-11). Both are HTML, not SVG, because both need a real text input — and both are **Korean-IME
 * safe**: the field is uncontrolled and the value is read on blur / explicit commit, never on every
 * `input`. A controlled React `value` re-render in the middle of a Hangul composition drops the
 * jamo being composed, which is the single most visible Korean-input bug in an editor like this.
 */
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { Pencil, X } from "lucide-react";
import type { Annot, Rect } from "../ipc/types";
import type { PageLayerContext } from "../viewer";
import { useT } from "../i18n/useT";
import { formatRelativeDay } from "../i18n";
import { PALETTE, useAnnotStore } from "../store/annotStore";
import { createAnnotation, deleteAnnotations, patchAnnotation, replyToAnnotation } from "./actions";
import { isoOf, threadReplies } from "./threads";
import { rgb } from "./shapes";

const POPOVER_W = 280;
const GAP_PX = 8;

function clampLeft(x: number, width: number): number {
  return Math.max(4, Math.min(x, Math.max(4, width - POPOVER_W - 4)));
}

/**
 * 메모 popover — swatches, 불투명도 and the note text, exactly the fast surface of UI_SPEC §7 —
 * plus the note's thread (P2): its replies and a reply box. It opens just below the note, so the
 * icon it belongs to stays visible while the thread grows.
 */
export function NotePopover({ ctx, annot }: { ctx: PageLayerContext; annot: Annot }) {
  const t = useT();
  const setEditing = useAnnotStore((s) => s.setEditing);
  const focusReply = useAnnotStore((s) => s.thread?.id === annot.id && s.thread.focusReply);
  const ref = useRef<HTMLTextAreaElement>(null);
  const box = ctx.rectToBox(annot.rect);

  useEffect(() => {
    if (focusReply) return; // 답글: the reply box takes the focus instead
    ref.current?.focus();
    ref.current?.setSelectionRange(ref.current.value.length, ref.current.value.length);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [annot.id]);

  const commit = () => {
    const value = ref.current?.value ?? "";
    if (value !== annot.contents) patchAnnotation(annot.page, annot.id, { contents: value });
  };

  return (
    <div
      className="annot-popover"
      data-testid="note-popover"
      style={{ left: clampLeft(box.x, ctx.width), top: box.y + box.h + GAP_PX, width: POPOVER_W }}
      onPointerDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") {
          commit();
          setEditing(null);
        }
      }}
    >
      <div className="annot-popover-head">
        <div className="swatches">
          {PALETTE.map((sw) => (
            <button
              key={sw.key}
              type="button"
              className="swatch"
              aria-label={t("a11y.colorSwatch", { name: t(sw.key) })}
              data-selected={sw.rgb.join() === annot.color.join() || undefined}
              style={{ background: rgb(sw.rgb) }}
              onClick={() => patchAnnotation(annot.page, annot.id, { color: sw.rgb })}
            />
          ))}
        </div>
        <button type="button" className="icon-btn" aria-label={t("common.close")} onClick={() => { commit(); setEditing(null); }}>
          <X size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </div>
      <textarea
        ref={ref}
        className="field annot-note-text"
        defaultValue={annot.contents}
        placeholder={t("prop.note.placeholder")}
        onBlur={commit}
      />
      <div className="annot-popover-foot">
        <span className="text-xs">{annot.author ?? ""}</span>
        <button
          type="button"
          className="btn quiet"
          onClick={() => {
            void deleteAnnotations(annot.page, [annot.id]).then((gone) => gone && setEditing(null));
          }}
        >
          {t("common.delete")}
        </button>
      </div>
      <ThreadSection root={annot} focusReply={focusReply} />
    </div>
  );
}

/**
 * P2 threads — the replies of `root`'s thread (flattened, oldest first) and a reply box. Every
 * reply can be edited in place (uncontrolled, IME-safe, written on blur) and deleted; the box
 * sends on its button or ⌘↵ / Ctrl+↵, as Settings' 작성자.
 */
export function ThreadSection({ root, focusReply }: { root: Annot; focusReply: boolean }) {
  const t = useT();
  const list = useAnnotStore((s) => s.byPage[root.page]);
  const replies = useMemo(() => threadReplies(list ?? [], root.id), [list, root.id]);
  const box = useRef<HTMLTextAreaElement>(null);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    if (focusReply) box.current?.focus();
  }, [focusReply, root.id]);

  const send = async () => {
    const text = box.current?.value ?? "";
    if (!text.trim() || busy) return;
    setBusy(true);
    const made = await replyToAnnotation(root.page, root.id, text);
    setBusy(false);
    if (made && box.current) box.current.value = "";
    box.current?.focus();
  };

  return (
    <div className="annot-thread" role="group" aria-label={t("annot.thread.label")}>
      {replies.length > 0 && (
        <ol className="annot-replies">
          {replies.map((reply) => (
            <ReplyItem key={reply.id} reply={reply} />
          ))}
        </ol>
      )}
      <div className="annot-reply-box">
        <textarea
          ref={box}
          className="field"
          rows={2}
          placeholder={t("annot.thread.placeholder")}
          aria-label={t("annot.thread.reply")}
          onKeyDown={(e) => {
            if (e.nativeEvent.isComposing) return;
            if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              void send();
            }
          }}
        />
        <button type="button" className="btn" disabled={busy} onClick={() => void send()}>
          {t("annot.thread.send")}
        </button>
      </div>
    </div>
  );
}

function ReplyItem({ reply }: { reply: Annot }) {
  const t = useT();
  const [editing, setEditing] = useState(false);
  const ref = useRef<HTMLTextAreaElement>(null);
  const when = formatRelativeDay(isoOf(reply.modified ?? reply.created));

  useLayoutEffect(() => {
    if (!editing) return;
    ref.current?.focus();
    ref.current?.setSelectionRange(ref.current.value.length, ref.current.value.length);
  }, [editing]);

  const commit = () => {
    const value = (ref.current?.value ?? "").trim();
    setEditing(false);
    if (value && value !== reply.contents) patchAnnotation(reply.page, reply.id, { contents: value });
  };

  return (
    <li className="annot-reply" data-testid="annot-reply">
      <div className="annot-reply-meta text-xs">
        <span className="annot-reply-author">{reply.author || t("annot.thread.noAuthor")}</span>
        {when && <span className="dim">{when}</span>}
        <span className="grow" />
        <button
          type="button"
          className="icon-btn"
          aria-label={t("annot.thread.edit")}
          onClick={() => setEditing(true)}
        >
          <Pencil size={12} strokeWidth={1.75} aria-hidden />
        </button>
        <button
          type="button"
          className="icon-btn"
          aria-label={t("annot.thread.delete")}
          onClick={() => void deleteAnnotations(reply.page, [reply.id])}
        >
          <X size={12} strokeWidth={1.75} aria-hidden />
        </button>
      </div>
      {editing ? (
        <textarea
          ref={ref}
          className="field"
          rows={2}
          defaultValue={reply.contents}
          aria-label={t("annot.thread.edit")}
          onBlur={commit}
          onKeyDown={(e) => {
            if (e.nativeEvent.isComposing) return;
            if (e.key === "Escape") {
              e.stopPropagation();
              setEditing(false);
            } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
              e.preventDefault();
              commit();
            }
          }}
        />
      ) : (
        <p className="annot-reply-text text-sm">{reply.contents}</p>
      )}
    </li>
  );
}

/**
 * 답글 on an annotation that is not a note (a highlight, a shape…): its thread in a popover under
 * it — the annotation's own 메모 as the first line, the replies, the reply box.
 */
export function ThreadPopover({ ctx, annot }: { ctx: PageLayerContext; annot: Annot }) {
  const t = useT();
  const setThread = useAnnotStore((s) => s.setThread);
  const focusReply = useAnnotStore((s) => s.thread?.id === annot.id && s.thread.focusReply);
  const box = ctx.rectToBox(annot.rect);
  return (
    <div
      className="annot-popover"
      data-testid="thread-popover"
      style={{ left: clampLeft(box.x, ctx.width), top: box.y + box.h + GAP_PX, width: POPOVER_W }}
      onPointerDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (e.key === "Escape") setThread(null);
      }}
    >
      <div className="annot-popover-head">
        <span className="text-sm annot-thread-title">{t("annot.thread.label")}</span>
        <button type="button" className="icon-btn" aria-label={t("common.close")} onClick={() => setThread(null)}>
          <X size={16} strokeWidth={1.75} aria-hidden />
        </button>
      </div>
      {annot.contents && <p className="annot-reply-text text-sm">{annot.contents}</p>}
      <ThreadSection root={annot} focusReply={focusReply} />
    </div>
  );
}

export interface TextDraft {
  page: number;
  rect: Rect;
  /** v0.3 A2: a 설명선 draft — the leader line (`/CL`, tip first); absent = a plain text box */
  callout?: number[];
}

/**
 * The 텍스트 상자 editor. For a draft it creates the annotation on commit (one undo step, and no
 * empty box is ever written); for an existing one it patches `text`.
 */
export function TextBoxEditor({
  ctx,
  draft,
  annot,
  onClose,
}: {
  ctx: PageLayerContext;
  draft?: TextDraft;
  annot?: Annot;
  onClose(): void;
}) {
  const t = useT();
  const style = useAnnotStore((s) => s.style);
  const ref = useRef<HTMLTextAreaElement>(null);
  const [composing, setComposing] = useState(false);
  const rect = annot?.rect ?? draft?.rect;
  const page = annot?.page ?? draft?.page;

  useLayoutEffect(() => {
    ref.current?.focus();
    const value = ref.current?.value ?? "";
    ref.current?.setSelectionRange(value.length, value.length);
  }, []);

  if (!rect || page === undefined) return null;
  const box = ctx.rectToBox(rect);
  const fontSize = (annot?.fontSize ?? style.fontSize) * ctx.scale;
  const colour = rgb(annot?.color ?? style.color);

  const commit = () => {
    const value = (ref.current?.value ?? "").replace(/\s+$/, "");
    if (annot) {
      if (value !== (annot.text ?? annot.contents)) patchAnnotation(annot.page, annot.id, { text: value, contents: value });
    } else if (value && draft?.callout?.length) {
      // v0.3 A2: 설명선 — the text box plus its leader, one annotation, one undo step
      void createAnnotation(page, {
        kind: "callout",
        rect,
        text: value,
        fontSize: style.fontSize,
        color: style.color,
        align: style.align,
        fillColor: style.fillColor,
        callout: draft.callout,
      });
    } else if (value) {
      void createAnnotation(page, {
        kind: "textbox",
        rect,
        text: value,
        fontSize: style.fontSize,
        color: style.color,
        align: style.align,
        fillColor: style.fillColor,
      });
    }
    onClose();
  };

  return (
    <textarea
      ref={ref}
      className="annot-text-editor"
      defaultValue={annot?.text ?? annot?.contents ?? ""}
      aria-label={t("tool.textbox")}
      style={{
        left: box.x,
        top: box.y,
        width: Math.max(box.w, 40),
        height: Math.max(box.h, fontSize * 1.6),
        fontSize,
        lineHeight: 1.25,
        color: colour,
      }}
      onPointerDown={(e) => e.stopPropagation()}
      onCompositionStart={() => setComposing(true)}
      onCompositionEnd={() => setComposing(false)}
      onBlur={commit}
      onKeyDown={(e) => {
        e.stopPropagation();
        if (composing) return;
        if (e.key === "Escape") {
          e.preventDefault();
          onClose();
        } else if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
          e.preventDefault();
          commit();
        }
      }}
    />
  );
}
