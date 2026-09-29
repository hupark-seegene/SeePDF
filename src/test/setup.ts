import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach } from "vitest";
import { cleanup, configure } from "@testing-library/react";
import { resetMock } from "../ipc/mock";
import { setLocale } from "../i18n";

// `findBy*` waits 1 s by default. On the Intel macOS runner the whole suite takes ~150 s and the
// first `React.lazy` dialog of a file (복구, cold module transform) took longer than that, which
// failed CI once with the dialog simply not there yet. A slow machine is not a failure.
configure({ asyncUtilTimeout: 5000 });

// jsdom has no ResizeObserver; the canvas stub uses one for 너비 맞춤.
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
}

beforeEach(async () => {
  resetMock();
  setLocale("ko");
  // v0.3 DR1: the tab strip is a module singleton like the stores it mirrors. Imported here, not at
  // the top: a static import would load `ipc/api` before a test file's `vi.mock("./env")` applies.
  (await import("../store/tabStore")).resetTabs();
});

afterEach(() => {
  cleanup();
});
