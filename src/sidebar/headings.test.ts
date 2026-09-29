/**
 * v0.3 P4 — the heading heuristic on synthetic lines: larger-than-body and numbered lines are
 * proposed with a level; running headers, page numbers and sentences are not.
 */
import { describe, expect, it } from "vitest";
import { bodySize, detectHeadings, numberingLevel, toOutlineNodes, type HeadingLine } from "./headings";

const body = (page: number, text: string, top = 500): HeadingLine => ({ page, text, sizePt: 10, top });
const big = (page: number, text: string, size: number, top = 700): HeadingLine => ({ page, text, sizePt: size, top });

function report(): HeadingLine[] {
  const lines: HeadingLine[] = [];
  for (let page = 0; page < 6; page++) {
    // a running header and a page number on every page
    lines.push(big(page, "2024 연간 보고서", 11, 820), body(page, String(page + 1), 30));
    for (let i = 0; i < 12; i++) lines.push(body(page, "본문 문장이 여기에 길게 이어집니다. 이 줄은 제목이 아닙니다.", 600 - i * 14));
  }
  lines.push(big(0, "제1장 서론", 18), big(0, "1.1 배경", 14, 650));
  lines.push(big(2, "제2장 방법", 18), body(2, "2.1 자료 수집", 640));
  lines.push(big(4, "부록", 18), body(4, "가. 용어 정리", 620));
  lines.push(big(5, "이 문장은 크게 쓰였지만 마침표로 끝나는 긴 문장입니다.", 16));
  return lines;
}

describe("headings", () => {
  it("knows the Korean and numeric numbering styles", () => {
    expect(numberingLevel("제3장 결론")).toBe(1);
    expect(numberingLevel("제 2 절 범위")).toBe(2);
    expect(numberingLevel("Ⅳ. 결과")).toBe(1);
    expect(numberingLevel("1. 개요")).toBe(1);
    expect(numberingLevel("1.2 목적")).toBe(2);
    expect(numberingLevel("1.2.3 세부")).toBe(3);
    expect(numberingLevel("가. 용어")).toBe(3);
    expect(numberingLevel("2024년 실적")).toBeNull();
  });

  it("finds the body size by characters, not by lines", () => {
    expect(bodySize(report())).toBe(10);
  });

  it("proposes headings with levels and skips headers, page numbers and sentences", () => {
    const found = detectHeadings(report());
    expect(found.map((h) => [h.title, h.page, h.level])).toEqual([
      ["제1장 서론", 0, 1],
      ["1.1 배경", 0, 2],
      ["제2장 방법", 2, 1],
      ["2.1 자료 수집", 2, 2],
      ["부록", 4, 1],
      ["가. 용어 정리", 4, 3],
    ]);
  });

  it("nests the proposals into outline nodes pointing at their lines", () => {
    const nodes = toOutlineNodes(detectHeadings(report()));
    expect(nodes.map((n) => n.title)).toEqual(["제1장 서론", "제2장 방법", "부록"]);
    expect(nodes[0].children.map((n) => n.title)).toEqual(["1.1 배경"]);
    expect(nodes[0]).toMatchObject({ page: 0, dest: { y: 700 }, open: true });
    expect(nodes[2].children[0]).toMatchObject({ title: "가. 용어 정리", page: 4 });
  });
});
