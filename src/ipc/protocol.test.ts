import { describe, expect, it } from "vitest";
import { originForOs, pageUrl, scaleKey, seepdfUrl, tileGrid, tileUrl, thumbUrl } from "./protocol";

describe("seepdf:// origin per platform (tauri spike §1)", () => {
  it("uses the custom scheme on macOS/Linux and the http form on Windows", () => {
    expect(originForOs("macos")).toBe("seepdf://localhost");
    expect(originForOs("linux")).toBe("seepdf://localhost");
    expect(originForOs("windows")).toBe("http://seepdf.localhost");
    expect(originForOs("windows", true)).toBe("https://seepdf.localhost");
  });

  it("builds a route with an escaped query and drops undefined values", () => {
    const url = seepdfUrl("/tile", { doc: "d1", gen: 3, page: 0, sk: 200, rot: 0, tx: 1, ty: 2, night: undefined }, originForOs("macos"));
    expect(url).toBe("seepdf://localhost/tile?doc=d1&gen=3&page=0&sk=200&rot=0&tx=1&ty=2");
    expect(seepdfUrl("thumb", { id: "a b/c" }, originForOs("windows"))).toBe("http://seepdf.localhost/thumb?id=a%20b%2Fc");
  });
});

describe("tile geometry helpers", () => {
  it("computes the scale key from zoom and dpr", () => {
    expect(scaleKey(118, 2)).toBe(236);
    expect(scaleKey(100, 1)).toBe(100);
  });

  it("splits a page into 512 px tiles", () => {
    expect(tileGrid(1024, 1536)).toEqual({ cols: 2, rows: 3 });
    expect(tileGrid(10, 10)).toEqual({ cols: 1, rows: 1 });
  });
});

describe("mock mode", () => {
  it("answers every image route with a PNG data URL instead of the protocol", () => {
    const common = { doc: "d1", gen: 1, page: 0, sk: 100, rot: 0 as const };
    expect(tileUrl({ ...common, tx: 0, ty: 0 })).toMatch(/^data:image\/png;base64,/);
    expect(pageUrl(common)).toMatch(/^data:image\/png;base64,/);
    expect(thumbUrl({ doc: "d1", gen: 1, page: 0, w: 120 })).toMatch(/^data:image\/png;base64,/);
  });
});
