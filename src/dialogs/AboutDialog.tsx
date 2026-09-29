/**
 * SeePDF 정보 (v0.3 pkg5, H1 + H11) — reached from ⋯ on both platforms (Windows has no menu bar, so
 * this is its only About) and from 도움말 on the welcome screen's shortcuts sheet. Shows the app
 * version, the PDFium build, OS / arch and the licences of what SeePDF ships with; 오픈 소스 라이선스
 * swaps the body for the full `THIRD_PARTY_NOTICES.txt` (`third_party_notices`) in a scrollable pane.
 *
 * Its own lazy chunk: nothing here is on the first screen.
 */
import { useEffect, useState } from "react";
import { Dialog } from "./Dialog";
import { useT } from "../i18n/useT";
import { useAppStore } from "../store/appStore";
import * as api from "../ipc/api";
import type { AppInfo } from "../ipc/types";
import "./about.css";

/** What the bundle carries, with its licence (README "Licences"; proper names, not UI strings). */
const COMPONENTS: { name: string; licence: string }[] = [
  { name: "PDFium", licence: "BSD-3-Clause" },
  { name: "pdfium-render", licence: "MIT / Apache-2.0" },
  { name: "Tauri", licence: "MIT / Apache-2.0" },
  { name: "React", licence: "MIT" },
  { name: "tesseract.js · Tesseract OCR", licence: "Apache-2.0" },
  { name: "Noto Sans KR (SeePDF-Hangul)", licence: "OFL-1.1" },
];

type Notices = { state: "loading" } | { state: "ready"; text: string } | { state: "missing"; detail: string };

export default function AboutDialog({ onClose }: { onClose(): void }) {
  const t = useT();
  const stored = useAppStore((s) => s.appInfo);
  const [info, setInfo] = useState<AppInfo | null>(stored);
  const [notices, setNotices] = useState<Notices | null>(null);

  useEffect(() => {
    if (info) return;
    let live = true;
    void api.appInfo().then((i) => live && setInfo(i)).catch(() => undefined);
    return () => {
      live = false;
    };
  }, [info]);

  const showNotices = () => {
    setNotices({ state: "loading" });
    void api
      .thirdPartyNotices()
      .then((text) => setNotices({ state: "ready", text }))
      .catch((e: unknown) => setNotices({ state: "missing", detail: e instanceof Error ? e.message : String(e) }));
  };

  if (notices) {
    return (
      <Dialog
        titleKey="about.licenses"
        size="lg"
        onClose={onClose}
        cancelKey="common.close"
        // v0.3.0 QA: 뒤로 is navigation, so it sits at the footer's leading edge (like 이미지 추가 /
        // 파일 추가 in other dialogs) and 닫기 keeps the trailing place the dialogs' last action has.
        footerExtra={
          <button type="button" className="btn" onClick={() => setNotices(null)}>
            {t("about.back")}
          </button>
        }
      >
        {notices.state === "loading" && <p className="text-sm dim">{t("about.licenses.loading")}</p>}
        {notices.state === "missing" && (
          <p className="text-sm" role="alert">
            {t("about.licenses.missing")}
          </p>
        )}
        {notices.state === "ready" && (
          <pre className="about-notices mono text-xs" tabIndex={0} aria-label={t("about.licenses")}>
            {notices.text}
          </pre>
        )}
      </Dialog>
    );
  }

  return (
    <Dialog
      titleKey="menu.help.about"
      size="md"
      onClose={onClose}
      cancelKey="common.close"
      secondary={{ labelKey: "about.licenses", onSelect: showNotices }}
    >
      <div className="about-head">
        <span className="text-lg about-name">{t("app.name")}</span>
        <span className="text-sm dim">{t("app.version", { version: info?.version ?? "…" })}</span>
      </div>
      <dl className="info-list">
        <div className="info-row">
          <dt className="text-sm dim">{t("about.engine")}</dt>
          <dd className="text-sm mono">{info ? `PDFium ${info.pdfiumVersion}` : "…"}</dd>
        </div>
        <div className="info-row">
          <dt className="text-sm dim">{t("about.platform")}</dt>
          <dd className="text-sm mono">{info ? `${info.os} / ${info.arch}${info.debug ? " (debug)" : ""}` : "…"}</dd>
        </div>
      </dl>
      <h3 className="text-sm about-subhead">{t("about.components")}</h3>
      <ul className="about-components text-sm">
        {COMPONENTS.map((c) => (
          <li key={c.name}>
            <span>{c.name}</span>
            <span className="dim mono text-xs">{c.licence}</span>
          </li>
        ))}
      </ul>
    </Dialog>
  );
}
