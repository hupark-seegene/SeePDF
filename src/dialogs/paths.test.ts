import { describe, expect, it } from "vitest";
import { baseName, dirName } from "./flows";

describe("dialogs.flows — path helpers", () => {
  it("dirName keeps the platform's separator and the root of a drive", () => {
    // Windows: `explorer /select,` and `split_document`'s outDir get a real Windows path back
    expect(dirName("C:\\Users\\hw\\Documents\\보고서.pdf")).toBe("C:\\Users\\hw\\Documents");
    // `C:` alone is the current directory on C:, not its root
    expect(dirName("C:\\a.pdf")).toBe("C:\\");
    expect(dirName("C:/a.pdf")).toBe("C:/");
    expect(dirName("\\\\server\\share\\scan.pdf")).toBe("\\\\server\\share");
    expect(dirName("/Users/veri/Documents/a.pdf")).toBe("/Users/veri/Documents");
    expect(dirName("/a.pdf")).toBe("/");
  });

  it("baseName takes either separator", () => {
    expect(baseName("C:\\Users\\hw\\보고서.pdf")).toBe("보고서.pdf");
    expect(baseName("/tmp/a.pdf")).toBe("a.pdf");
  });
});
