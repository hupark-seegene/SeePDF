#!/usr/bin/env node
/**
 * THIRD_PARTY_NOTICES.txt (v0.3 pkg5, H11): the licences of everything SeePDF ships with, which
 * the MIT / Apache / BSD licences require to travel with the binary. Shown in the app under
 * SeePDF 정보 ▸ 오픈 소스 라이선스 (`third_party_notices`), bundled as
 * `resources/notices/THIRD_PARTY_NOTICES.txt` (`bundle.resources` in the tauri configs).
 *
 *   node scripts/gen-notices.mjs            # write src-tauri/resources/notices/THIRD_PARTY_NOTICES.txt
 *   node scripts/gen-notices.mjs --check    # …and fail unless pdfium-render, lopdf, tauri and react are in it
 *   node scripts/gen-notices.mjs --out <file>
 *
 * Sources:
 *   - Rust: `cargo metadata` — every crate reachable from `seepdf` through normal (not dev / build)
 *     dependencies, on every platform the resolver knows (a superset of what one bundle links);
 *     the `license` field plus the LICENSE* / COPYING* / NOTICE* files next to its Cargo.toml.
 *   - npm: the production tree (`npm ls --omit=dev --all --parseable`); `license` plus LICENSE files.
 *   - The files the bundle carries as-is: PDFium's LICENSE, the font's OFL.txt, the OCR assets' LICENSE.
 *
 * Identical licence texts are printed once, under the list of packages that use them, which keeps
 * the file a few hundred kB instead of several MB. Run before `tauri build` (release.yml); the file
 * is generated, so it is gitignored.
 */
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const check = args.includes("--check");
const outIndex = args.indexOf("--out");
const OUT = outIndex >= 0 ? args[outIndex + 1] : join(ROOT, "src-tauri", "resources", "notices", "THIRD_PARTY_NOTICES.txt");
/** What `--check` insists on: the engine binding, the lopdf rewriter, the shell, the UI. */
const REQUIRED = ["pdfium-render", "lopdf", "tauri", "react"];

const LICENSE_FILE = /^(licen[cs]e|copying|notice|unlicense)([-._].*)?$/i;

function licenceFiles(dir) {
  let names = [];
  try {
    names = readdirSync(dir);
  } catch {
    return [];
  }
  return names
    .filter((n) => LICENSE_FILE.test(n))
    .sort()
    .map((n) => {
      try {
        return readFileSync(join(dir, n), "utf8").replace(/\r\n/g, "\n").trim();
      } catch {
        return null;
      }
    })
    .filter((t) => t);
}

/** Rust crates reachable from the app crate through normal dependencies. */
function rustPackages() {
  const cargo = process.env.CARGO || "cargo";
  const json = execFileSync(
    cargo,
    ["metadata", "--format-version", "1", "--locked", "--manifest-path", join(ROOT, "src-tauri", "Cargo.toml")],
    { encoding: "utf8", maxBuffer: 256 * 1024 * 1024, stdio: ["ignore", "pipe", "inherit"] },
  );
  const meta = JSON.parse(json);
  const byId = new Map(meta.packages.map((p) => [p.id, p]));
  const nodes = new Map(meta.resolve.nodes.map((n) => [n.id, n]));
  const root = meta.resolve.root;
  const seen = new Set([root]);
  const queue = [root];
  while (queue.length) {
    const node = nodes.get(queue.shift());
    for (const dep of node?.deps ?? []) {
      const normal = dep.dep_kinds.some((k) => k.kind === null);
      if (normal && !seen.has(dep.pkg)) {
        seen.add(dep.pkg);
        queue.push(dep.pkg);
      }
    }
  }
  seen.delete(root);
  return [...seen].map((id) => {
    const p = byId.get(id);
    return {
      ecosystem: "crate",
      name: p.name,
      version: p.version,
      licence: p.license ?? (p.license_file ? `see ${p.license_file}` : "unknown"),
      url: p.repository ?? p.homepage ?? "",
      texts: licenceFiles(dirname(p.manifest_path)),
    };
  });
}

/** npm packages in the production tree (the frontend bundle and the OCR runtime). */
function npmPackages() {
  const npm = process.platform === "win32" ? "npm.cmd" : "npm";
  let out = "";
  try {
    out = execFileSync(npm, ["ls", "--omit=dev", "--all", "--parseable"], {
      cwd: ROOT,
      encoding: "utf8",
      maxBuffer: 64 * 1024 * 1024,
      stdio: ["ignore", "pipe", "ignore"],
      shell: process.platform === "win32",
    });
  } catch (e) {
    // `npm ls` exits 1 on an extraneous package but still prints the tree
    out = e.stdout ?? "";
  }
  const dirs = [...new Set(out.split(/\r?\n/).filter(Boolean))].filter((d) => d !== ROOT);
  return dirs
    .map((dir) => {
      let pkg;
      try {
        pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
      } catch {
        return null;
      }
      const licence = typeof pkg.license === "string" ? pkg.license : pkg.license?.type ?? "unknown";
      const repo = typeof pkg.repository === "string" ? pkg.repository : pkg.repository?.url ?? pkg.homepage ?? "";
      return { ecosystem: "npm", name: pkg.name, version: pkg.version, licence, url: repo, texts: licenceFiles(dir) };
    })
    .filter((p) => p);
}

/** Files the bundle ships verbatim. */
function bundledFiles() {
  const files = [
    ["PDFium (libpdfium, pdfium-binaries build)", "src-tauri/resources/pdfium/LICENSE.pdfium", "BSD-3-Clause AND MIT"],
    ["SeePDF-Hangul.ttf (subset of Noto Sans KR)", "src-tauri/resources/fonts/OFL.txt", "OFL-1.1"],
    ["tesseract.js runtime, tesseract.js-core, kor/eng traineddata", "public/ocr/LICENSE.txt", "Apache-2.0"],
  ];
  return files
    .filter(([, rel]) => existsSync(join(ROOT, rel)))
    .map(([name, rel, licence]) => ({
      ecosystem: "bundled",
      name,
      version: "",
      licence,
      url: rel,
      texts: [readFileSync(join(ROOT, rel), "utf8").replace(/\r\n/g, "\n").trim()],
    }));
}

const APACHE_KEY = "\u0000apache-2.0";

function isApache2(text) {
  return /Apache License\s+Version 2\.0, January 2004/i.test(text) && /TERMS AND CONDITIONS FOR USE/i.test(text);
}

function label(p) {
  return p.version ? `${p.name} ${p.version}` : p.name;
}

function build() {
  const packages = [
    ...bundledFiles(),
    ...rustPackages().sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version)),
    ...npmPackages().sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version)),
  ];
  const texts = new Map(); // hash → { text, users[] }
  for (const p of packages) {
    for (const raw of p.texts) {
      // The Apache License 2.0 itself is one text; the dozens of copies differ only in whitespace and
      // an appendix. It is printed once — a crate's own NOTICE / copyright file still gets its entry.
      const text = isApache2(raw) ? APACHE_KEY : raw;
      const hash = createHash("sha256").update(text.replace(/\s+/g, " ")).digest("hex");
      if (!texts.has(hash)) texts.set(hash, { text: text === APACHE_KEY ? raw : text, users: [] });
      texts.get(hash).users.push(label(p));
    }
  }
  const lines = [];
  lines.push("SeePDF — third-party notices");
  lines.push("");
  lines.push("SeePDF includes the software listed below. Each component is used under its own licence;");
  lines.push("the full licence texts follow the list. Generated by scripts/gen-notices.mjs.");
  lines.push("");
  for (const [title, eco] of [
    ["Bundled files", "bundled"],
    ["Rust crates", "crate"],
    ["npm packages", "npm"],
  ]) {
    const group = packages.filter((p) => p.ecosystem === eco);
    if (!group.length) continue;
    lines.push(`== ${title} (${group.length}) ==`);
    lines.push("");
    for (const p of group) lines.push(`${label(p)} — ${p.licence}${p.url ? ` — ${p.url}` : ""}`);
    lines.push("");
  }
  lines.push("== Licence texts ==");
  lines.push("");
  for (const { text, users } of texts.values()) {
    lines.push("-".repeat(78));
    lines.push(`Used by: ${users.join(", ")}`);
    lines.push("-".repeat(78));
    lines.push("");
    lines.push(text);
    lines.push("");
  }
  return { text: lines.join("\n") + "\n", packages };
}

const { text, packages } = build();
mkdirSync(dirname(OUT), { recursive: true });
writeFileSync(OUT, text);
console.log(`[notices] ${packages.length} components, ${(text.length / 1024).toFixed(0)} kB → ${OUT}`);

if (check) {
  const names = new Set(packages.map((p) => p.name));
  const missing = REQUIRED.filter((n) => !names.has(n));
  if (missing.length) {
    console.error(`[notices] missing: ${missing.join(", ")}`);
    process.exit(1);
  }
  if (!text.includes("PDFium")) {
    console.error("[notices] the PDFium licence is not in the file (run `npm run fetch:pdfium` first)");
    process.exit(1);
  }
  console.log(`[notices] check ok: ${REQUIRED.join(", ")} present`);
}
