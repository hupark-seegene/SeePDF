/**
 * P1-12: the per-page view nonce of `set_annotations_hidden` reaches the bitmap URLs (`vn`), so a
 * hidden annotation is a new URL the webview's immutable cache has never seen — and a page with
 * no nonce keeps the exact URL it had before.
 */
import { afterEach, describe, expect, it } from "vitest";
import { pageUrl, setMockAssetResolver, tileUrl, type Query } from "./protocol";

function queryOf(build: () => void): Query {
  let seen: Query = {};
  setMockAssetResolver((_route, query) => {
    seen = query;
    return "data:,";
  });
  build();
  return seen;
}

afterEach(() => setMockAssetResolver(null));

describe("view nonce in bitmap URLs", () => {
  const common = { doc: "d1", gen: 4, page: 2, sk: 150, rot: 0 as const };

  it("carries vn when the page has one", () => {
    expect(queryOf(() => pageUrl({ ...common, vn: 7 })).vn).toBe(7);
    expect(queryOf(() => tileUrl({ ...common, tx: 1, ty: 0, vn: 7 })).vn).toBe(7);
  });

  it("leaves it out otherwise", () => {
    expect(queryOf(() => pageUrl(common)).vn).toBeUndefined();
    expect(queryOf(() => tileUrl({ ...common, tx: 0, ty: 0 })).vn).toBeUndefined();
  });
});
