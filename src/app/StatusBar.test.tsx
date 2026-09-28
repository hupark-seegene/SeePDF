/**
 * The zoom combo must show the zoom the view is at. A custom zoom off `ZOOM_STEPS` (slider,
 * pinch, a position restored from 최근 항목 — 184 % in the real-app smoke) used to have no
 * matching <option>, so the <select> fell back to its first one and read "25%".
 */
import { describe, expect, it } from "vitest";
import { act, fireEvent, render, screen } from "@testing-library/react";
import { StatusBar } from "./StatusBar";
import { useViewStore } from "../store/viewStore";
import { useDocStore } from "../store/docStore";

function combo(container: HTMLElement): HTMLSelectElement {
  const el = container.querySelector<HTMLSelectElement>("select.zoom-combo");
  if (!el) throw new Error("no zoom combo");
  return el;
}

describe("StatusBar zoom combo", () => {
  it("shows a custom zoom that is not a preset", () => {
    const { container } = render(<StatusBar />);
    act(() => useViewStore.getState().setZoom(184));
    const select = combo(container);
    expect(select.value).toBe("184");
    expect(select.selectedOptions[0]?.textContent).toBe("184%");
  });

  it("keeps presets and fit modes on their own options", () => {
    const { container } = render(<StatusBar />);
    act(() => useViewStore.getState().setZoom(150));
    expect(combo(container).value).toBe("150");
    expect(combo(container).querySelectorAll('option[value="150"]')).toHaveLength(1);
    act(() => useViewStore.getState().setZoomMode("fit-width"));
    expect(combo(container).value).toBe("fit-width");
  });
});

describe("StatusBar ◀ ▶ in 두 쪽", () => {
  it("steps a whole spread each way, from either page of the spread", async () => {
    await useDocStore.getState().open("/tmp/sample.pdf");
    const info = useDocStore.getState().info!;
    useDocStore.setState({ info: { ...info, pageCount: 40, pages: Array.from({ length: 40 }, (_, i) => ({ ...info.pages[0], index: i })) } });
    act(() => useViewStore.setState({ layout: "two", currentPage: 10 }));
    render(<StatusBar />);
    fireEvent.click(screen.getByRole("button", { name: /이전 페이지/ }));
    expect(useViewStore.getState().currentPage).toBe(8);
    act(() => useViewStore.setState({ currentPage: 11 }));
    fireEvent.click(screen.getByRole("button", { name: /다음 페이지/ }));
    expect(useViewStore.getState().currentPage).toBe(12);
    // on the last spread there is nowhere further to go
    act(() => useViewStore.setState({ currentPage: 38 }));
    expect(screen.getByRole("button", { name: /다음 페이지/ })).toBeDisabled();
    act(() => useViewStore.setState({ layout: "continuous", currentPage: 0 }));
  });
});
