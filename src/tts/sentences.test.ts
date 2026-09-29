/**
 * V4 (v0.3): the Korean-aware sentence splitter behind 읽어 주기's sentence queue and highlight.
 */
import { describe, expect, it } from "vitest";
import { speakableText, splitSentences, textSentences } from "./sentences";

const texts = (text: string) => splitSentences(text).map((s) => text.slice(s.start, s.end));

describe("splitSentences", () => {
  it("ends a sentence at . ? ! … followed by a space or the end", () => {
    expect(texts("안녕하세요. 반갑습니다! 괜찮으세요? 네… 좋아요")).toEqual([
      "안녕하세요.",
      "반갑습니다!",
      "괜찮으세요?",
      "네…",
      "좋아요",
    ]);
    expect(texts("It works. Does it? Yes!")).toEqual(["It works.", "Does it?", "Yes!"]);
  });

  it("keeps decimals, versions and list numbers inside their sentence", () => {
    expect(texts("원주율은 3.14입니다. v0.3에서 바뀌었습니다.")).toEqual(["원주율은 3.14입니다.", "v0.3에서 바뀌었습니다."]);
    expect(texts("2. 주석 도구를 씁니다. a. 첫째 항목입니다.")).toEqual(["2. 주석 도구를 씁니다.", "a. 첫째 항목입니다."]);
  });

  it("CJK full stops end a sentence with no space after them; closing quotes stay with it", () => {
    expect(texts("끝났다。다음 문장")).toEqual(["끝났다。", "다음 문장"]);
    expect(texts("그가 말했다 \"좋다.\" 그리고 갔다.")).toEqual(["그가 말했다 \"좋다.\"", "그리고 갔다."]);
  });

  it("a short line (heading, list item) ends at its line break; a wrapped paragraph does not", () => {
    const text = [
      "1장 개요",
      "이 문서는 긴 문단이 여러 줄에 걸쳐 이어질 때 문장이 줄 끝에서",
      "잘리지 않는지 확인하기 위한 예시 문단으로 충분히 길게 씁니다.",
    ].join("\n");
    expect(texts(text)).toEqual([
      "1장 개요",
      "이 문서는 긴 문단이 여러 줄에 걸쳐 이어질 때 문장이 줄 끝에서\n잘리지 않는지 확인하기 위한 예시 문단으로 충분히 길게 씁니다.",
    ]);
    // a blank line always ends one
    expect(texts("첫 문단 끝나지 않은 문장\n\n둘째 문단")).toEqual(["첫 문단 끝나지 않은 문장", "둘째 문단"]);
  });

  it("textSentences folds whitespace and drops empty pieces", () => {
    expect(textSentences("  하나.   둘.\n\n ").map((s) => s.text)).toEqual(["하나.", "둘."]);
    expect(textSentences(" \n ")).toEqual([]);
  });
});

describe("speakableText", () => {
  it("joins a word PDFium split with a soft hyphen (U+0002) at a line end", () => {
    expect(speakableText("more difficult to com\u0002\r\npile than")).toBe("more difficult to compile than");
    expect(speakableText("discov­\nered paths")).toBe("discovered paths");
    expect(speakableText("a stray \u0002 mark")).toBe("a stray mark");
  });
});
