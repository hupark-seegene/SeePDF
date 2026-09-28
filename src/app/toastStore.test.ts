import { beforeEach, describe, expect, it } from "vitest";
import { toast, useToastStore } from "./toastStore";

const last = () => useToastStore.getState().toasts.at(-1)!;

describe("toast timeouts", () => {
  beforeEach(() => useToastStore.setState({ toasts: [] }));

  it("defaults the timeout when the caller does not give one (not `undefined`, which dismisses at once)", () => {
    toast("edit.flow.pushed", { pt: 10 });
    expect(last().timeoutMs).toBe(4500);
    toast("error.generic", undefined, { tone: "danger" });
    expect(last().timeoutMs).toBe(8000);
  });

  it("keeps an explicit timeout, including 0 (stays until closed)", () => {
    toast("status.saved", undefined, { tone: "success", timeoutMs: 2200 });
    expect(last().timeoutMs).toBe(2200);
    toast("status.saved", undefined, { timeoutMs: 0 });
    expect(last().timeoutMs).toBe(0);
  });
});
