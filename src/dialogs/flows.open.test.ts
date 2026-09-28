import { beforeEach, describe, expect, it, vi } from "vitest";
import { waitFor } from "@testing-library/react";
import { openPath } from "./flows";
import { useDialogStore } from "./dialogState";
import { useDocStore } from "../store/docStore";
import { useToastStore } from "../app/toastStore";
import { mock } from "../ipc/mock";

const A = "/Users/veri/Documents/SeePDF-샘플.pdf";
const B = "/tmp/other.pdf";

/** Is `docId` still loaded in the (mock) engine? */
async function loaded(docId: string): Promise<boolean> {
  return mock.getDocument({ docId }).then(
    () => true,
    () => false,
  );
}

describe("dialogs.flows.open — the replaced document is released", () => {
  beforeEach(() => {
    useDocStore.setState({ docId: null, info: null, outline: [], status: "empty", error: null });
    useDialogStore.getState().closeAll();
    useToastStore.setState({ toasts: [] });
  });

  it("the first open closes nothing", async () => {
    const close = vi.spyOn(mock, "closeDocument");
    expect(await openPath(A)).not.toBeNull();
    expect(close).not.toHaveBeenCalled();
  });

  it("opening B closes A in the engine, once", async () => {
    const a = (await openPath(A))!;
    const close = vi.spyOn(mock, "closeDocument");
    const b = (await openPath(B))!;
    expect(b.docId).not.toBe(a.docId);
    expect(close).toHaveBeenCalledTimes(1);
    expect(close).toHaveBeenCalledWith({ docId: a.docId });
    expect(await loaded(a.docId)).toBe(false);
    expect(await loaded(b.docId)).toBe(true);
    expect(useDocStore.getState().docId).toBe(b.docId);
    expect(useDocStore.getState().status).toBe("ready");
  });

  it("a failed open keeps A displayed and open", async () => {
    const a = (await openPath(A))!;
    const close = vi.spyOn(mock, "closeDocument");
    expect(await openPath("/tmp/damaged.pdf")).toBeNull();
    expect(close).not.toHaveBeenCalled();
    expect(await loaded(a.docId)).toBe(true);
    const doc = useDocStore.getState();
    expect(doc.docId).toBe(a.docId);
    expect(doc.info?.docId).toBe(a.docId);
    // the toast reports the failure; the window is back to showing A, not an error state
    expect(doc.status).toBe("ready");
    expect(doc.error).toBeNull();
    expect(useToastStore.getState().toasts.map((t) => t.messageKey)).toContain("error.openFailed");
  });

  it("a failed open on an empty window still reports the error", async () => {
    expect(await openPath("/tmp/damaged.pdf")).toBeNull();
    expect(useDocStore.getState().info).toBeNull();
    expect(useDocStore.getState().status).toBe("error");
  });

  it("취소 on B's password prompt keeps A displayed and open", async () => {
    const a = (await openPath(A))!;
    const close = vi.spyOn(mock, "closeDocument");
    const opening = openPath("/tmp/encrypted.pdf");
    await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe("password"));
    const entry = useDialogStore.getState().stack.at(-1)!;
    (entry.props.resolve as (v: string | null) => void)(null);
    expect(await opening).toBeNull();
    expect(close).not.toHaveBeenCalled();
    expect(await loaded(a.docId)).toBe(true);
    expect(useDocStore.getState().docId).toBe(a.docId);
    expect(useDocStore.getState().status).toBe("ready");
  });

  it("a password-protected B closes A only once it has actually opened", async () => {
    const a = (await openPath(A))!;
    const close = vi.spyOn(mock, "closeDocument");
    const opening = openPath("/tmp/encrypted.pdf");
    await waitFor(() => expect(useDialogStore.getState().stack.at(-1)?.name).toBe("password"));
    expect(close).not.toHaveBeenCalled();
    const entry = useDialogStore.getState().stack.at(-1)!;
    (entry.props.resolve as (v: string | null) => void)("secret");
    const b = (await opening)!;
    expect(b.docId).not.toBe(a.docId);
    expect(close).toHaveBeenCalledTimes(1);
    expect(close).toHaveBeenCalledWith({ docId: a.docId });
  });
});
