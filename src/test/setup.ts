import "@testing-library/jest-dom/vitest";
import { afterEach, beforeEach } from "vitest";
import { cleanup } from "@testing-library/react";
import { resetMock } from "../ipc/mock";
import { setLocale } from "../i18n";

// jsdom has no ResizeObserver; the canvas stub uses one for 너비 맞춤.
if (typeof globalThis.ResizeObserver === "undefined") {
  globalThis.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
}

beforeEach(() => {
  resetMock();
  setLocale("ko");
});

afterEach(() => {
  cleanup();
});
