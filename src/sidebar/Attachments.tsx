/**
 * 첨부 파일 (v0.3 S4): the document-level attachments — name and size, 저장… for each, and in 편집
 * mode 파일 첨부… and 삭제 (one undo step each). The tab only appears when the document has
 * attachments or 편집 mode is on (`SidebarFrame`). Lazy-loaded.
 */
import { useEffect, useState } from "react";
import { Download, Paperclip, Plus, Trash2 } from "lucide-react";
import * as api from "../ipc/api";
import { formatBytes } from "../i18n";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { toast } from "../app/toastStore";
import { IconButton } from "../app/IconButton";
import type { AttachmentInfo } from "../ipc/types";
import "./attachments.css";

function detail(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

async function reveal(path: string) {
  const { revealAction } = await import("../dialogs/flows");
  return revealAction(path);
}

export default function Attachments() {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const editing = useAppStore((s) => s.mode === "edit");
  const [items, setItems] = useState<AttachmentInfo[] | null>(null);
  const [busy, setBusy] = useState(false);
  const docId = info?.docId;
  const generation = info?.docGeneration;
  const canModify = info?.permissions.modify ?? false;

  useEffect(() => {
    if (!docId) return;
    let live = true;
    api
      .listAttachments({ docId })
      .then((list) => live && setItems(list))
      .catch(() => live && setItems([]));
    return () => {
      live = false;
    };
  }, [docId, generation]);

  if (!info) return null;

  const run = async (work: () => Promise<AttachmentInfo[] | void>) => {
    setBusy(true);
    try {
      const next = await work();
      if (next) {
        setItems(next);
        await useDocStore.getState().refresh();
      }
    } catch (e) {
      toast("error.generic", undefined, { tone: "danger", detail: detail(e) });
    } finally {
      setBusy(false);
    }
  };

  const save = (a: AttachmentInfo) =>
    run(async () => {
      const ext = a.name.includes(".") ? a.name.slice(a.name.lastIndexOf(".") + 1) : "";
      const path = await api.saveFileDialog({
        defaultPath: a.name,
        filters: ext ? [{ name: ext.toUpperCase(), extensions: [ext] }] : [],
      });
      if (!path) return;
      await api.saveAttachment({ docId: info.docId, index: a.index, path });
      toast("attachments.saved", undefined, { tone: "success", actions: [await reveal(path)] });
    });

  const add = () =>
    run(async () => {
      const picked = await api.openFileDialog({ multiple: false, filters: [] });
      if (!picked?.length) return;
      return api.addAttachment({ docId: info.docId, path: picked[0] });
    });

  const remove = (a: AttachmentInfo) => run(() => api.deleteAttachment({ docId: info.docId, index: a.index }));

  return (
    <div className="attachments-panel">
      {editing && (
        <div className="attachments-bar">
          <button type="button" className="btn quiet text-sm" disabled={busy || !canModify} onClick={() => void add()}>
            <Plus size={14} aria-hidden /> {t("attachments.add")}
          </button>
        </div>
      )}
      {items === null ? (
        <p className="empty">{t("common.loading")}</p>
      ) : items.length === 0 ? (
        <p className="empty">{t("attachments.empty")}</p>
      ) : (
        <ul className="attachments-list" aria-label={t("sidebar.tab.attachments")}>
          {items.map((a) => (
            <li key={`${a.index}-${a.name}`} className="attachments-row">
              <Paperclip size={14} aria-hidden className="dim" />
              <span className="attachments-name text-base" title={a.name}>
                {a.name}
              </span>
              <span className="text-xs dim">{formatBytes(a.size)}</span>
              <IconButton icon={Download} label={t("attachments.save")} disabled={busy} onClick={() => void save(a)} />
              {editing && (
                <IconButton
                  icon={Trash2}
                  label={t("attachments.delete")}
                  disabled={busy || !canModify}
                  onClick={() => void remove(a)}
                />
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
