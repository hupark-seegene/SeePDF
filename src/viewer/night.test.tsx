/**
 * 야간 모드 (P1-10): 끄기 → 어둡게 → 세피아 from ⌃⌘N, the 보기 menu id and the status-bar moon; the
 * filter reaches the bitmap layer only; the engine is asked for a night bitmap (transparent clear
 * colour, no colour transform — `render_night_is_transparent_not_inverted` on the Rust side) and the
 * CSS paper under it is exactly what the filter makes of white.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { NIGHT_MODES, nextNight, readNight, useViewStore } from "../store/viewStore";
import { MENU_IDS, shortcutFor } from "../keys/keymap";
import { setMockAssetResolver } from "../ipc/protocol";
import { readFileSync } from "node:fs";
import { join } from "node:path";

// Read from disk (vitest runs from the repo root): `css: false` empties CSS imports, `?raw` included.
const viewerCss = readFileSync(join(process.cwd(), "src/viewer/viewer.css"), "utf8");
const tokensCss = readFileSync(join(process.cwd(), "src/styles/tokens.css"), "utf8");

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";
const BLANK = "data:image/png;base64,iVBORw0KGgo=";

/** Every `seepdf://` request the viewer and the sidebar make, with its query. */
const requests: { route: string; query: Record<string, unknown> }[] = [];

beforeEach(() => {
  requests.length = 0;
  setMockAssetResolver((route, query) => {
    requests.push({ route, query: { ...query } });
    return BLANK;
  });
  useViewStore.setState({ night: "off" });
  useAppStore.setState({ os: "macos", mode: "read", tool: "select" });
});

afterEach(() => {
  useViewStore.setState({ night: "off" });
});

async function openSample() {
  render(<App />);
  await waitFor(() => expect(useAppStore.getState().ready).toBe(true));
  await useDocStore.getState().open(SAMPLE);
  await waitFor(() => expect(document.querySelectorAll(".page-shell").length).toBeGreaterThan(0));
}

function nightOf(selector: string): string | null {
  return document.querySelector(selector)?.getAttribute("data-night") ?? null;
}

describe("view.night", () => {
  it("cycles 끄기 → 어둡게 → 세피아 → 끄기", () => {
    expect(NIGHT_MODES).toEqual(["off", "dark", "sepia"]);
    expect(nextNight("off")).toBe("dark");
    expect(nextNight("dark")).toBe("sepia");
    expect(nextNight("sepia")).toBe("off");
    useViewStore.getState().cycleNight();
    useViewStore.getState().cycleNight();
    expect(useViewStore.getState().night).toBe("sepia");
  });

  it("is reachable from ⌃⌘N / Ctrl+Shift+N and the native 보기 menu", () => {
    expect(shortcutFor("view.night", "macos")).toBe("⌃⌘N");
    expect(shortcutFor("view.night", "windows")).toBe("Ctrl+Shift+N");
    expect(MENU_IDS).toContain("view.night");
  });

  it("⌃⌘N and the status-bar moon filter the bitmap layer only, and ask the engine for night bitmaps", async () => {
    await openSample();
    expect(nightOf(".canvas.viewer")).toBeNull();
    expect(nightOf(".page-bitmaps")).toBeNull();

    fireEvent.keyDown(window, { key: "n", code: "KeyN", metaKey: true, ctrlKey: true });
    await waitFor(() => expect(useViewStore.getState().night).toBe("dark"));
    await waitFor(() => expect(nightOf(".page-bitmaps")).toBe("dark"));
    expect(nightOf(".canvas.viewer")).toBe("dark");
    // The overlays are siblings of the filtered layer, never inside it: nothing is inverted twice.
    expect(document.querySelector(".page-bitmaps .page-marks, .page-bitmaps .page-annots, .page-bitmaps .page-forms, .page-bitmaps .page-surface")).toBeNull();
    for (const el of document.querySelectorAll(".page-shell > :not(.page-bitmaps)")) {
      expect(el.hasAttribute("data-night")).toBe(false);
    }

    const moon = screen.getByRole("button", { name: "야간 모드: 어둡게" });
    expect(moon).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(moon);
    expect(useViewStore.getState().night).toBe("sepia");
    await waitFor(() => expect(nightOf(".page-bitmaps")).toBe("sepia"));
    fireEvent.click(screen.getByRole("button", { name: "야간 모드: 세피아" }));
    expect(useViewStore.getState().night).toBe("off");
    await waitFor(() => expect(nightOf(".page-bitmaps")).toBeNull());
    expect(screen.getByRole("button", { name: "야간 모드: 끄기" })).toHaveAttribute("aria-pressed", "false");

    // Page bitmaps were requested with `night=1` (both modes share one bitmap) and without it by
    // day; thumbnails are never night-rendered (the sidebar is not the tile layer).
    const pages = requests.filter((r) => r.route === "/page" || r.route === "/tile");
    expect(pages.some((r) => r.query.night === 1)).toBe(true);
    expect(pages.some((r) => r.query.night === undefined)).toBe(true);
    expect(pages.every((r) => r.query.night === undefined || r.query.night === 1)).toBe(true);
    const thumbs = requests.filter((r) => r.route === "/thumb");
    expect(thumbs.length).toBeGreaterThan(0);
    expect(thumbs.every((r) => r.query.night === undefined)).toBe(true);
  });

  it("is remembered across launches: every change goes to Settings.night, and a start restores it (Stage 8)", async () => {
    const { mock } = await import("../ipc/mock");
    const patch = vi.spyOn(mock, "setSettings");
    useViewStore.getState().cycleNight();
    expect(useViewStore.getState().night).toBe("dark");
    await waitFor(() => expect(patch).toHaveBeenCalledWith({ patch: { night: "dark" } }));
    useViewStore.getState().setNight("sepia");
    await waitFor(() => expect(patch).toHaveBeenLastCalledWith({ patch: { night: "sepia" } }));
    // setting the same mode again writes nothing
    const calls = patch.mock.calls.length;
    useViewStore.getState().setNight("sepia");
    expect(patch.mock.calls.length).toBe(calls);
    patch.mockRestore();

    // a new window: the view starts dark-less, settings arrive → the remembered mode is back
    useViewStore.setState({ night: "off" });
    useAppStore.setState({ settings: null, ready: false });
    await useAppStore.getState().bootstrap();
    expect(useAppStore.getState().settings?.night).toBe("sepia");
    expect(useViewStore.getState().night).toBe("sepia");

    // a settings file from before Stage 8 (no `night`, or junk) starts with it off
    useViewStore.setState({ night: "off" });
    useAppStore.setState({ settings: null });
    useAppStore.setState({ settings: { ...(useAppStore.getState().settings ?? {}), night: "bogus" } as never });
    expect(useViewStore.getState().night).toBe("off");
    expect(readNight(undefined)).toBe("off");
    expect(readNight("dark")).toBe("dark");
  });
});

// ---------------------------------------------------------------------------
// The stylesheet contract
// ---------------------------------------------------------------------------

/** `selector → declarations` for every top-level rule (the viewer's CSS has no nesting). */
function rules(css: string): { selector: string; body: string }[] {
  const out: { selector: string; body: string }[] = [];
  const clean = css.replace(/\/\*[\s\S]*?\*\//g, "");
  for (const m of clean.matchAll(/([^{}@]+)\{([^{}]*)\}/g)) out.push({ selector: m[1].trim(), body: m[2] });
  return out;
}

function hexOf(css: string, token: string): string {
  const m = new RegExp(`${token}:\\s*(#[0-9a-f]{6})`, "i").exec(css);
  if (!m) throw new Error(`${token} is not a hex colour in tokens.css`);
  return m[1].toLowerCase();
}

describe("night-mode CSS", () => {
  const viewer = rules(viewerCss);

  it("filters .page-bitmaps and nothing else", () => {
    const filtered = viewer.filter((r) => /(^|[\s;])filter\s*:/.test(r.body));
    expect(filtered.map((r) => r.selector).sort()).toEqual([
      '.viewer .page-bitmaps[data-night="dark"]',
      '.viewer .page-bitmaps[data-night="sepia"]',
    ]);
  });

  it("puts the page on night paper, with the page tokens re-scoped for the overlays", () => {
    const dark = viewer.find((r) => r.selector === '.viewer[data-night="dark"]')?.body ?? "";
    const sepia = viewer.find((r) => r.selector === '.viewer[data-night="sepia"]')?.body ?? "";
    expect(dark).toMatch(/--page-paper:\s*var\(--night-paper\)/);
    expect(dark).toMatch(/--page-ink:\s*var\(--night-ink\)/);
    expect(sepia).toMatch(/--page-paper:\s*var\(--sepia-paper\)/);
    // the shell paints the paper the transparent night bitmap is composited over
    const shell = viewer.find((r) => r.selector === ".viewer .page-shell")?.body ?? "";
    expect(shell).toMatch(/background:\s*var\(--page-paper\)/);
  });

  it("makes the dark paper exactly what the filter turns white into (and the ink what it turns black into)", () => {
    const dark = viewer.find((r) => r.selector === '.viewer .page-bitmaps[data-night="dark"]')?.body ?? "";
    const amount = Number(/invert\(([\d.]+)\)/.exec(dark)?.[1]);
    expect(amount).toBeGreaterThan(0.5);
    const filtered = (v: number) => 255 * (v * (1 - 2 * amount) + amount);
    const grey = (hex: string) => {
      const [r, g, b] = [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16));
      expect(r === g && g === b, `${hex} is a grey`).toBe(true);
      return r;
    };
    // hue-rotate leaves greys alone, so white and black only see the invert (±1 for rounding)
    expect(Math.abs(grey(hexOf(tokensCss, "--night-paper")) - filtered(1))).toBeLessThanOrEqual(1);
    expect(Math.abs(grey(hexOf(tokensCss, "--night-ink")) - filtered(0))).toBeLessThanOrEqual(1);
  });

  it("multiplies the sepia page onto its paper and keeps multiply marks on it; screen marks on dark only", () => {
    const sepia = viewer.find((r) => r.selector === '.viewer .page-bitmaps[data-night="sepia"]')?.body ?? "";
    expect(sepia).toMatch(/mix-blend-mode:\s*multiply/);
    const screenMarks = viewer.filter((r) => /mix-blend-mode:\s*screen/.test(r.body)).map((r) => r.selector);
    expect(screenMarks).toEqual(['.viewer[data-night="dark"] .mark']);
  });
});
