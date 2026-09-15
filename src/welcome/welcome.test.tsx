import { beforeEach, describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { Welcome } from "./Welcome";
import DialogHost from "../dialogs/DialogHost";
import { openPaths } from "../dialogs/flows";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useContextMenuStore } from "../app/contextMenuStore";
import { useDialogStore } from "../dialogs/dialogState";

describe("welcome.recents", () => {
  // every test starts on the welcome screen: no document, no dialog left over
  beforeEach(() => {
    useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
    useDialogStore.getState().closeAll();
  });

  it("lists the recents with their folder, date, page count and size, pinned first", async () => {
    render(<Welcome />);
    expect(await screen.findByText("SeePDF-샘플.pdf")).toBeInTheDocument();
    expect(screen.getByText("계약서 초안.pdf")).toBeInTheDocument();
    expect(screen.getAllByText("/Users/veri/Documents").length).toBeGreaterThan(0);
    expect(screen.getByText(/14쪽/)).toBeInTheDocument();

    const names = screen.getAllByRole("button").map((b) => b.textContent ?? "");
    const first = names.find((n) => n.includes(".pdf"));
    expect(first).toContain("SeePDF-샘플.pdf"); // the pinned entry
  });

  it("filters the list", async () => {
    render(<Welcome />);
    await screen.findByText("SeePDF-샘플.pdf");
    fireEvent.change(screen.getByLabelText("최근 항목 검색"), { target: { value: "계약" } });
    expect(screen.getByText("계약서 초안.pdf")).toBeInTheDocument();
    expect(screen.queryByText("SeePDF-샘플.pdf")).toBeNull();
  });

  it("opens a recent and restores its reading position", async () => {
    render(<Welcome />);
    const card = await screen.findByRole("button", { name: /tracemonkey\.pdf/ });
    fireEvent.click(card);
    await waitFor(() => expect(useDocStore.getState().info?.name).toBe("tracemonkey.pdf"));
    const { useViewStore } = await import("../store/viewStore");
    expect(useViewStore.getState().currentPage).toBe(2); // recents fixture: lastPage 2
    expect(useViewStore.getState().zoomPercent).toBe(150);
    expect(useViewStore.getState().layout).toBe("single");
  });

  it("offers 하나로 합치기 when several files are dropped", async () => {
    render(<DialogHost />);
    const done = openPaths(["/tmp/a.pdf", "/tmp/b.pdf", "/tmp/c.pdf"]);
    expect(await screen.findByText("여러 파일을 어떻게 열까요?")).toBeInTheDocument();
    expect(screen.getByText("a.pdf")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "하나로 합치기" }));
    await done;
    await waitFor(() => expect(useDocStore.getState().info?.name).toBe("merged.pdf"));
    expect(useDocStore.getState().info?.path).toBeNull();
  });

  it("각각 열기 opens the first file in this window", async () => {
    render(<DialogHost />);
    const done = openPaths(["/tmp/one.pdf", "/tmp/two.pdf"]);
    await screen.findByText("여러 파일을 어떻게 열까요?");
    fireEvent.click(screen.getByRole("button", { name: "각각 열기" }));
    await done;
    await waitFor(() => expect(useDocStore.getState().info?.name).toBe("one.pdf"));
  });

  it("a single dropped file opens directly, with no prompt", async () => {
    render(<DialogHost />);
    await openPaths(["/tmp/solo.pdf"]);
    await waitFor(() => expect(useDocStore.getState().info?.name).toBe("solo.pdf"));
    expect(screen.queryByText("여러 파일을 어떻게 열까요?")).toBeNull();
  });

  it("the recent card has a context menu with 즐겨찾기 and 목록에서 제거", async () => {
    render(<Welcome />);
    const card = await screen.findByRole("button", { name: /계약서 초안\.pdf/ });
    fireEvent.contextMenu(card);
    const menu = useContextMenuStore.getState().menu;
    expect(menu?.items.map((i) => ("labelKey" in i ? i.labelKey : "sep"))).toEqual([
      "common.open",
      "menu.file.revealInFinder",
      "welcome.recent.pin",
      "sep",
      "welcome.recent.remove",
    ]);
  });

  it("opening a document records it in the recents", async () => {
    await openPaths(["/tmp/fresh.pdf"]);
    await waitFor(() => expect(useAppStore.getState().recents[0]?.name).toBe("fresh.pdf"));
    expect(useAppStore.getState().recents[0].lastPage).toBe(0);
  });
});
