/**
 * 보안 (P1-2 / P1-3): 암호 설정 writes a protected copy, 암호 제거 writes an unlocked copy, and
 * 메타데이터 제거 edits the open document. Lazy-loaded from `DialogHost`.
 */
import { useState } from "react";
import * as api from "../ipc/api";
import { useT } from "../i18n/useT";
import { useDocStore } from "../store/docStore";
import { toast } from "../app/toastStore";
import { Dialog, Row } from "./Dialog";
import { message, revealAction, suggestName } from "./flows";
import { buildSetPasswordArgs, EMPTY_SECURITY_FORM, validatePasswords, type SecurityForm } from "./security";

type Busy = null | "set" | "remove" | "meta";

export default function SecurityDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const info = useDocStore((s) => s.info);
  const [form, setForm] = useState<SecurityForm>(EMPTY_SECURITY_FORM);
  const [busy, setBusy] = useState<Busy>(null);
  if (!info) return null;

  const check = validatePasswords(form);
  const patch = (p: Partial<SecurityForm>) => setForm((f) => ({ ...f, ...p }));

  async function run(kind: Exclude<Busy, null>, work: () => Promise<boolean>) {
    setBusy(kind);
    try {
      if (await work()) onClose();
    } catch (e) {
      toast("error.generic", undefined, { tone: "danger", detail: message(e) });
    } finally {
      setBusy(null);
    }
  }

  const setPassword = () =>
    run("set", async () => {
      const outPath = await api.saveFileDialog({ defaultPath: suggestName(info.name, "protected") });
      if (!outPath) return false;
      await api.setPassword(buildSetPasswordArgs(info.docId, outPath, form));
      toast("dialog.security.passwordSet", undefined, { tone: "success", actions: [revealAction(outPath)] });
      return true;
    });

  const removePassword = () =>
    run("remove", async () => {
      const outPath = await api.saveFileDialog({ defaultPath: suggestName(info.name, "unlocked") });
      if (!outPath) return false;
      await api.removePassword({ docId: info.docId, outPath });
      toast("dialog.security.passwordRemoved", undefined, { tone: "success", actions: [revealAction(outPath)] });
      return true;
    });

  const removeMetadata = () =>
    run("meta", async () => {
      await api.removeMetadata({ docId: info.docId });
      await useDocStore.getState().refresh();
      toast("dialog.security.metadataRemoved", undefined, { tone: "success" });
      return true;
    });

  const mismatch = check.error === "openMismatch" || check.error === "ownerMismatch";
  const pw = (value: string, onChange: (v: string) => void, labelKey: string) => (
    <input
      className="field grow"
      type="password"
      autoComplete="new-password"
      aria-label={t(labelKey)}
      placeholder={t(labelKey)}
      value={value}
      onChange={(e) => onChange(e.target.value)}
    />
  );

  return (
    <Dialog
      titleKey="security.title"
      size="lg"
      onClose={onClose}
      primary={{ labelKey: "dialog.security.setAction", onSelect: setPassword, disabled: !check.ok || busy !== null }}
    >
      <section className="sec-section" aria-label={t("security.password.set")}>
        <h3 className="text-base sec-head">{t("security.password.set")}</h3>
        <p className="dlg-hint text-xs">{t("dialog.security.copyHint")}</p>
        <Row labelKey="security.password.open">
          <div className="inline-row">
            {pw(form.openPassword, (v) => patch({ openPassword: v }), "security.password.open")}
            {pw(form.openConfirm, (v) => patch({ openConfirm: v }), "security.password.confirm")}
          </div>
        </Row>
        <Row labelKey="security.password.owner" hintKey="dialog.security.ownerHint">
          <div className="inline-row">
            {pw(form.ownerPassword, (v) => patch({ ownerPassword: v }), "security.password.owner")}
            {pw(form.ownerConfirm, (v) => patch({ ownerConfirm: v }), "security.password.confirm")}
          </div>
        </Row>
        {(mismatch || check.error === "ownerRequired") && (
          <p className="dlg-hint danger text-xs" role="alert">
            {t(mismatch ? "security.password.mismatch" : "dialog.security.ownerRequired")}
          </p>
        )}
        <Row labelKey="security.permissions">
          {(
            [
              ["print", "security.permission.print"],
              ["copy", "security.permission.copy"],
              ["modify", "security.permission.modify"],
              ["annotate", "security.permission.annotate"],
            ] as const
          ).map(([k, key]) => (
            <label className="dlg-check text-base" key={k}>
              <input type="checkbox" checked={form[k]} onChange={(e) => patch({ [k]: e.target.checked })} />
              <span>{t(key)}</span>
            </label>
          ))}
        </Row>
        <Row labelKey="security.encryption">
          <span className="text-sm sec-value">{t("dialog.security.aes")}</span>
        </Row>
      </section>

      <section className="sec-section" aria-label={t("security.password.remove")}>
        <h3 className="text-base sec-head">{t("security.password.remove")}</h3>
        <div className="inline-row">
          <p className="dlg-hint text-xs grow">
            {t(info.encrypted ? "dialog.security.removeHint" : "dialog.security.notEncrypted")}
          </p>
          <button type="button" className="btn" disabled={!info.encrypted || busy !== null} onClick={removePassword}>
            {t("dialog.security.removeAction")}
          </button>
        </div>
      </section>

      <section className="sec-section" aria-label={t("security.removeMetadata")}>
        <h3 className="text-base sec-head">{t("security.removeMetadata")}</h3>
        <div className="inline-row">
          <p className="dlg-hint text-xs grow">
            {t(info.encrypted ? "dialog.security.protectedHint" : "security.removeMetadataHint")}
          </p>
          <button type="button" className="btn" disabled={info.encrypted || busy !== null} onClick={removeMetadata}>
            {t("security.removeMetadata")}
          </button>
        </div>
      </section>
    </Dialog>
  );
}
