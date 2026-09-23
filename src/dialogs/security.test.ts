import { describe, expect, it } from "vitest";
import { buildSetPasswordArgs, EMPTY_SECURITY_FORM, permissionsOf, validatePasswords, type SecurityForm } from "./security";

const form = (p: Partial<SecurityForm>): SecurityForm => ({ ...EMPTY_SECURITY_FORM, ...p });

describe("security.validatePasswords", () => {
  it("both blank is invalid", () => {
    expect(validatePasswords(EMPTY_SECURITY_FORM)).toEqual({ ok: false, error: "empty" });
  });
  it("flags a mismatched confirm", () => {
    expect(validatePasswords(form({ openPassword: "a", openConfirm: "b" })).error).toBe("openMismatch");
    expect(validatePasswords(form({ ownerPassword: "a", ownerConfirm: "" })).error).toBe("ownerMismatch");
  });
  it("accepts an open password alone, an owner password alone, or both", () => {
    expect(validatePasswords(form({ openPassword: "a", openConfirm: "a" })).ok).toBe(true);
    expect(validatePasswords(form({ ownerPassword: "o", ownerConfirm: "o" })).ok).toBe(true);
    expect(
      validatePasswords(form({ openPassword: "a", openConfirm: "a", ownerPassword: "o", ownerConfirm: "o" })).ok,
    ).toBe(true);
  });
});

describe("security.validatePasswords with restrictions", () => {
  it("refuses a restriction without a distinct owner password", () => {
    // blank owner: the open password would become the owner password and the restriction is void
    expect(validatePasswords(form({ openPassword: "a", openConfirm: "a", copy: false })).error).toBe("ownerRequired");
    // owner equal to the open password: same problem
    expect(
      validatePasswords(form({ openPassword: "a", openConfirm: "a", ownerPassword: "a", ownerConfirm: "a", print: false }))
        .error,
    ).toBe("ownerRequired");
  });
  it("accepts a restriction with a separate owner password, with or without an open password", () => {
    expect(validatePasswords(form({ ownerPassword: "o", ownerConfirm: "o", modify: false })).ok).toBe(true);
    expect(
      validatePasswords(form({ openPassword: "a", openConfirm: "a", ownerPassword: "o", ownerConfirm: "o", annotate: false }))
        .ok,
    ).toBe(true);
  });
  it("still lets the open password stand in as owner when nothing is restricted", () => {
    expect(validatePasswords(form({ openPassword: "a", openConfirm: "a" })).ok).toBe(true);
  });
});

describe("security.buildSetPasswordArgs", () => {
  it("falls back to the open password as the owner password", () => {
    const a = buildSetPasswordArgs("doc-1", "/x.pdf", form({ openPassword: "pw", openConfirm: "pw" }));
    expect(a.userPassword).toBe("pw");
    expect(a.ownerPassword).toBe("pw");
    expect(a.outPath).toBe("/x.pdf");
  });
  it("omits the user password when only an owner password is set", () => {
    const a = buildSetPasswordArgs("doc-1", "/x.pdf", form({ ownerPassword: "o", ownerConfirm: "o" }));
    expect(a.userPassword).toBeUndefined();
    expect(a.ownerPassword).toBe("o");
  });
  it("maps the checkboxes onto Permissions and keeps explicit false", () => {
    expect(permissionsOf(EMPTY_SECURITY_FORM)).toEqual({ print: true, extractText: true, modify: true, annotate: true });
    const a = buildSetPasswordArgs("doc-1", "/x.pdf", form({ openPassword: "p", openConfirm: "p", copy: false, annotate: false }));
    expect(a.permissions).toEqual({ print: true, extractText: false, modify: true, annotate: false });
  });
});
