import { describe, expect, it } from "vitest";
import { findAll, findKeyword, findPatterns, mergeMatches } from "./redactPatterns";

const texts = (text: string, ...kinds: Parameters<typeof findPatterns>[1][number][]) =>
  findPatterns(text, kinds).map((m) => m.text);

describe("주민등록번호", () => {
  it("matches with and without the hyphen", () => {
    expect(texts("주민번호: 900101-1234567 입니다", "rrn")).toEqual(["900101-1234567"]);
    expect(texts("주민번호 9001012234567.", "rrn")).toEqual(["9001012234567"]);
    expect(texts("외국인 020304 - 4123456", "rrn")).toEqual(["020304 - 4123456"]);
  });

  it("does not match a date, an invalid birth date or a longer digit run", () => {
    expect(texts("작성일 2024-01-01", "rrn")).toEqual([]);
    expect(texts("2024-01-01 12:00", "rrn", "phone", "account")).toEqual([]);
    expect(texts("901301-1234567", "rrn")).toEqual([]); // month 13
    expect(texts("900132-1234567", "rrn")).toEqual([]); // day 32
    expect(texts("900101-5234567", "rrn")).toEqual([]); // 7th digit 5
    expect(texts("1900101-12345678", "rrn")).toEqual([]);
  });
});

describe("전화번호", () => {
  it("matches mobile and landline numbers in the usual spellings", () => {
    expect(
      texts("휴대폰 010-1234-5678, 사무실 02-123-4567, 031.765.4321, (02) 987-6543, 01098765432", "phone"),
    ).toEqual(["010-1234-5678", "02-123-4567", "031.765.4321", "(02) 987-6543", "01098765432"]);
    expect(texts("+82 10-2222-3333", "phone")).toEqual(["+82 10-2222-3333"]);
    expect(texts("070-8888-9999", "phone")).toEqual(["070-8888-9999"]);
  });

  it("does not match dates, prices or pieces of longer numbers", () => {
    expect(texts("2024-01-01", "phone")).toEqual([]);
    expect(texts("1,234,567원", "phone")).toEqual([]);
    expect(texts("110-123-456789", "phone")).toEqual([]);
  });
});

describe("이메일", () => {
  it("matches an ASCII address inside Korean text without swallowing the particle", () => {
    expect(texts("문의는 hong.gildong@example.co.kr로 보내 주세요", "email")).toEqual(["hong.gildong@example.co.kr"]);
    expect(texts("연락처hong@example.com입니다", "email")).toEqual(["hong@example.com"]);
  });

  it("matches a Korean address", () => {
    expect(texts("메일: 홍길동@회사.한국 (업무용)", "email")).toEqual(["홍길동@회사.한국"]);
    expect(texts("메일: 홍길동@example.kr", "email")).toEqual(["홍길동@example.kr"]);
  });

  it("needs a real domain", () => {
    expect(texts("@username, a@b, x@y.1", "email")).toEqual([]);
  });
});

describe("계좌번호", () => {
  it("matches grouped and bare account numbers", () => {
    expect(texts("국민 123456-78-901234, 신한 110-123-456789, 우리 1002345678901", "account")).toEqual([
      "123456-78-901234",
      "110-123-456789",
      "1002345678901",
    ]);
  });

  it("leaves phone numbers, dates and short numbers alone", () => {
    expect(texts("010-1234-5678 2024-01-01 12-34-56 01012345678", "account")).toEqual([]);
  });
});

describe("keywords and merging", () => {
  it("finds a keyword across generated line breaks, case-insensitively", () => {
    expect(findKeyword("담당자 홍길동, 홍길동 님", "홍길동").map((m) => m.start)).toEqual([4, 9]);
    expect(findKeyword("Andreas\r\nGal and andreas gal", "andreas gal").map((m) => m.text)).toEqual([
      "Andreas\r\nGal",
      "andreas gal",
    ]);
    expect(findKeyword("ABC abc", "abc", true).map((m) => m.start)).toEqual([4]);
    expect(findKeyword("a.b axb", "a.b").map((m) => m.text)).toEqual(["a.b"]);
    expect(findKeyword("anything", "   ")).toEqual([]);
  });

  it("merges overlapping matches of different kinds", () => {
    const merged = findAll("번호 9001011234567 끝", { patterns: ["rrn", "account"] });
    expect(merged).toHaveLength(1);
    expect(merged[0]).toMatchObject({ text: "9001011234567", start: 3, end: 16 });
    expect(
      mergeMatches([
        { start: 0, end: 5, kind: "keyword", text: "abcde" },
        { start: 3, end: 8, kind: "email", text: "defgh" },
      ]),
    ).toEqual([{ start: 0, end: 8, kind: "keyword", text: "abcdefgh" }]);
  });
});
