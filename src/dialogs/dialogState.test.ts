/**
 * Two prompts of the same kind (bug hunt 2026-09): 문서 비교's password prompt for B and a dropped
 * encrypted file's, or the ⌘W unsaved prompt and the window's. The stack keeps one entry per name,
 * so the second replaces the first — and the first caller must hear "cancelled", not hang forever.
 */
import { beforeEach, describe, expect, it } from "vitest";
import { askConfirm, askPassword, askUnsaved, closeDialog, useDialogStore } from "./dialogState";

type Resolver<T> = (value: T) => void;

function top<T>(): Resolver<T> {
  const stack = useDialogStore.getState().stack;
  return stack[stack.length - 1].props.resolve as Resolver<T>;
}

async function settled(p: Promise<unknown>): Promise<boolean> {
  let done = false;
  void p.then(() => (done = true));
  await new Promise((r) => setTimeout(r, 0));
  return done;
}

beforeEach(() => {
  useDialogStore.getState().closeAll();
});

describe("ask* prompts of the same kind", () => {
  it("a second password prompt settles the first with 취소 (null)", async () => {
    const first = askPassword("compare-B.pdf");
    const second = askPassword("dropped.pdf");
    expect(useDialogStore.getState().stack.filter((e) => e.name === "password")).toHaveLength(1);
    expect(await first).toBeNull();

    top<string | null>()("secret");
    expect(await second).toBe("secret");
    expect(useDialogStore.getState().stack).toHaveLength(0);
  });

  it("a second unsaved prompt answers the first 'cancel'; a confirm answers false", async () => {
    const first = askUnsaved("a.pdf");
    const second = askUnsaved("a.pdf");
    expect(await first).toBe("cancel");
    top<string>()("dontSave");
    expect(await second).toBe("dontSave");

    const c1 = askConfirm({ titleKey: "common.close", bodyKey: "common.close" });
    const c2 = askConfirm({ titleKey: "common.close", bodyKey: "common.close" });
    expect(await c1).toBe(false);
    top<boolean>()(true);
    expect(await c2).toBe(true);
  });

  it("a prompt closed from outside (closeAll, closeDialog) settles as cancelled too", async () => {
    const pw = askPassword("x.pdf");
    useDialogStore.getState().closeAll();
    expect(await pw).toBeNull();

    const unsaved = askUnsaved("y.pdf");
    closeDialog("unsaved");
    expect(await unsaved).toBe("cancel");
  });

  it("answering settles once, and only that prompt", async () => {
    const pw = askPassword("x.pdf");
    const resolve = top<string | null>();
    resolve("one");
    resolve("two");
    expect(await pw).toBe("one");
    expect(await settled(askPassword("z.pdf"))).toBe(false);
  });
});
