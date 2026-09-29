#!/usr/bin/env node
/**
 * Release configuration gate (v0.3 pkg5: H12, H13, O4, H11). Runs in ci.yml's frontend job and in
 * release.yml's version job, before anything is built. Asserts:
 *
 *   H12  the Windows installers are Korean + English (NSIS language selector, ko-KR + en-US MSIs),
 *        the main installer embeds the WebView2 bootstrapper, and release.yml also builds the
 *        WebView2-offline variant `SeePDF_<version>_x64-offline-setup.exe`;
 *   H13  release.yml builds and tests x86_64-apple-darwin, and `release-latest-json.mjs` — run
 *        against a fake asset list — emits both darwin entries and never the offline installer;
 *   O4   the macOS bundle requires 13.0 (WebKit's WASM SIMD, which the OCR core needs);
 *   H11  every platform config bundles `resources/notices/*`.
 *
 *   node scripts/check-release-config.mjs
 */
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const json = (rel) => JSON.parse(readFileSync(join(ROOT, rel), "utf8"));
const errors = [];
const expect = (ok, message) => {
  if (!ok) errors.push(message);
};

const base = json("src-tauri/tauri.conf.json");
const win = json("src-tauri/tauri.windows.conf.json");
const mac = json("src-tauri/tauri.macos.conf.json");
const release = readFileSync(join(ROOT, ".github/workflows/release.yml"), "utf8");

// H12 — Windows installers
const nsis = win.bundle?.windows?.nsis ?? {};
expect(JSON.stringify(nsis.languages) === JSON.stringify(["Korean", "English"]), `nsis.languages must be ["Korean","English"], is ${JSON.stringify(nsis.languages)}`);
expect(nsis.displayLanguageSelector === true, "nsis.displayLanguageSelector must be true");
const wix = win.bundle?.windows?.wix?.language;
expect(Array.isArray(wix) && wix.includes("ko-KR") && wix.includes("en-US"), `wix.language must list ko-KR and en-US, is ${JSON.stringify(wix)}`);
expect(win.bundle?.windows?.webviewInstallMode?.type === "embedBootstrapper", "the main installer must embed the WebView2 bootstrapper");
expect(/offlineInstaller/.test(release), "release.yml must build the WebView2 offlineInstaller variant");
expect(/_x64-offline-setup\.exe/.test(release), "release.yml must publish SeePDF_<version>_x64-offline-setup.exe");

// H13 — Intel macOS
const intelRows = release.match(/target:\s*x86_64-apple-darwin/g) ?? [];
expect(intelRows.length >= 2, `release.yml must test and bundle x86_64-apple-darwin (found ${intelRows.length} matrix rows)`);

// O4 — macOS 13
const min = base.bundle?.macOS?.minimumSystemVersion ?? "0";
expect(Number.parseFloat(min) >= 13, `bundle.macOS.minimumSystemVersion must be ≥ 13.0 (WASM SIMD for OCR), is ${min}`);

// H11 — notices bundled everywhere
for (const [name, conf] of [["tauri.conf.json", base], ["tauri.windows.conf.json", win], ["tauri.macos.conf.json", mac]]) {
  const resources = conf.bundle?.resources ?? [];
  expect(resources.includes("resources/notices/*"), `${name}: bundle.resources must include resources/notices/*`);
}

// H13 — latest.json from a fake asset list
const dir = mkdtempSync(join(tmpdir(), "seepdf-latest-json-"));
try {
  const assets = [
    "SeePDF_0.3.0_x64-setup.exe",
    "SeePDF_0.3.0_x64-offline-setup.exe",
    "SeePDF_0.3.0_x64_ko-KR.msi",
    "SeePDF_0.3.0_x64_en-US.msi",
    "SeePDF_0.3.0_aarch64.app.tar.gz",
    "SeePDF_0.3.0_x64.app.tar.gz",
  ];
  for (const a of assets) {
    writeFileSync(join(dir, a), "bundle");
    writeFileSync(join(dir, `${a}.sig`), `sig-of-${a}`);
  }
  execFileSync(process.execPath, [
    join(ROOT, "scripts/release-latest-json.mjs"),
    "--dir", dir, "--version", "0.3.0", "--tag", "v0.3.0", "--repo", "owner/SeePDF",
  ], { stdio: "ignore" });
  const manifest = JSON.parse(readFileSync(join(dir, "latest.json"), "utf8"));
  const p = manifest.platforms ?? {};
  for (const key of ["darwin-aarch64", "darwin-x86_64", "darwin-aarch64-app", "darwin-x86_64-app", "windows-x86_64", "windows-x86_64-nsis", "windows-x86_64-msi"]) {
    expect(p[key], `latest.json lacks ${key}`);
  }
  expect(p["darwin-x86_64"]?.url?.endsWith("SeePDF_0.3.0_x64.app.tar.gz"), "darwin-x86_64 must point at the x64 app bundle");
  expect(p["darwin-aarch64"]?.url?.endsWith("SeePDF_0.3.0_aarch64.app.tar.gz"), "darwin-aarch64 must point at the aarch64 app bundle");
  expect(p["windows-x86_64"]?.url?.endsWith("SeePDF_0.3.0_x64-setup.exe"), "windows-x86_64 must be the plain NSIS installer");
  expect(!JSON.stringify(manifest).includes("offline"), "the offline installer must never be an updater target");
} catch (e) {
  errors.push(`release-latest-json.mjs: ${e.message}`);
} finally {
  rmSync(dir, { recursive: true, force: true });
}

if (errors.length) {
  for (const e of errors) console.error(`[release-config] ${e}`);
  process.exit(1);
}
console.log("[release-config] ok: installers ko+en, WebView2 embed + offline variant, macOS 13+, arm64 + x64 assets, notices bundled");
