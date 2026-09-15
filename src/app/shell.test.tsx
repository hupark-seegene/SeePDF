import { describe, expect, it } from "vitest";
import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import App from "../App";
import { useAppStore } from "../store/appStore";
import { useDocStore } from "../store/docStore";
import { useViewStore } from "../store/viewStore";

const SAMPLE = "/Users/veri/Documents/SeePDF-샘플.pdf";

async function openSample() {
  await useDocStore.getState().open(SAMPLE);
  await waitFor(() => expect(useDocStore.getState().info).not.toBeNull());
}

describe("app shell", () => {
  it("shows the Korean welcome screen with recents from the API", async () => {
    useDocStore.setState({ docId: null, info: null, outline: [], status: "empty" });
    render(<App />);
    expect(await screen.findByText("최근 항목")).toBeInTheDocument();
    await waitFor(() => expect(screen.getByText("SeePDF-샘플.pdf")).toBeInTheDocument());
    expect(screen.getByRole("button", { name: /파일 열기/ })).toBeInTheDocument();
  });

  it("opens a document and renders the chrome around it", async () => {
    render(<App />);
    await waitFor(() => expect(useAppStore.getState().ready).toBe(true));
    await openSample();

    expect(await screen.findByRole("tab", { name: "주석" })).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "문서 보기 영역" })).toBeInTheDocument();
    // The Stage 1 viewer is virtualised: only the rows inside the ±1.5/2.5-screen window are
    // mounted, so the assertion is "the canvas painted pages", not "it painted all of them".
    await waitFor(() => expect(document.querySelectorAll(".page-shell").length).toBeGreaterThan(0));
    expect(document.querySelector('.page-shell[data-page="0"]')).toBeInTheDocument();
    expect(screen.getByLabelText("페이지 번호")).toHaveValue("1");
    expect(screen.getByText("저장됨")).toBeInTheDocument();
  });

  it("switches mode from the mode switcher and shows that mode's tool strip", async () => {
    render(<App />);
    await openSample();
    fireEvent.click(await screen.findByRole("tab", { name: "주석" }));
    await waitFor(() => expect(useAppStore.getState().mode).toBe("annotate"));
    expect(await screen.findByRole("toolbar", { name: "도구" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "형광펜" })).toBeInTheDocument();
  });

  it("drives the keymap: ⌘2 arms 주석 mode, H picks the highlighter", async () => {
    useAppStore.setState({ os: "macos", mode: "read", tool: "select" });
    render(<App />);
    await openSample();

    fireEvent.keyDown(window, { key: "2", code: "Digit2", metaKey: true });
    await waitFor(() => expect(useAppStore.getState().mode).toBe("annotate"));
    fireEvent.keyDown(window, { key: "h", code: "KeyH" });
    await waitFor(() => expect(useAppStore.getState().tool).toBe("highlight"));
  });

  it("zooms from the status bar", async () => {
    render(<App />);
    await openSample();
    useViewStore.setState({ zoomMode: "custom", zoomPercent: 100 });
    fireEvent.click(await screen.findByRole("button", { name: "확대" }));
    await waitFor(() => expect(useViewStore.getState().zoomPercent).toBe(125));
  });
});
