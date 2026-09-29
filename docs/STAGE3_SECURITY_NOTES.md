# Stage 3 — security and metadata (P1-1, P1-2, P1-3)

Status: landed in the working tree, verified, not committed yet (2026-09-23).

## 1. What landed

* **P1-1 metadata edit.** 문서 정보 (⌘I) now has editable 제목 / 작성자 / 주제 / 키워드 fields,
  locale-formatted 만든 날짜 / 수정한 날짜, and an 적용 button. 보안 → 메타데이터 제거 deletes `/Info`
  and the XMP packet. Both are one undo step.
* **P1-2 remove password.** 보안 → 암호 제거 writes an `-unlocked` copy
  (`FPDF_SaveAsCopy` + `FPDF_REMOVE_SECURITY = 4`). It is enabled only when the document is encrypted.
* **P1-3 password protect.** 보안 → 암호 설정 takes an open password and a permissions password, each
  with a confirm field, plus 4 permission checkboxes. It writes an AES-256 (V5 / R6) `-protected`
  copy and shows a toast with 'Finder에서 보기'.
* The 보안 dialog is reached from the native menu (`tools.security`) and the ⋯ overflow menu. It is
  lazy-loaded in its own chunk (1.48 kB gz).
* **Registry read fixes:** pdfium-render cannot report R5/R6 security (AES-256 files looked
  unencrypted with every permission), and it asked for `ModificationDate` instead of `ModDate`. Both
  are now read with the raw PDFium API (`FPDF_GetSecurityHandlerRevision`, `FPDF_GetDocPermissions`,
  `FPDF_GetMetaText`).

## 2. Design decisions

**Byte-level mutate primitive (`registry::mutate_bytes`).** PDFium reads `/Info` and encryption but
cannot write either. So a metadata edit runs in these steps:
1. Push an undo snapshot.
2. `save::serialize` (appearance streams and form kill-focus included).
3. Rewrite the bytes with lopdf.
4. `save::verify_bytes` reopens the result with PDFium and checks the page count.
5. `replace` reloads the document; the generation is bumped and `doc-changed` is sent
   (`changedPages: "all"`, `structure: false`).
If any step fails, the undo entry is dropped and the snapshot is reloaded. The `doc-changed` code is
now one `announce` helper shared by `mutate`, `mutate_bytes` and `undo`.

**lopdf round-trip, PDFium verification.** lopdf 0.45 (default features off) only rewrites bytes
that PDFium produced. PDFium is always the judge of the result:
* Metadata: the rewritten file must reopen with the same page count.
* `set_password`: the copy must open with the user password (or with none) and with the owner
  password. When a user password is set, it must *fail* to open without one. Only after these
  checks does `write_atomic` touch `outPath`. The open document is never modified. An encrypted input
  is serialised with `RemoveSecurity` first, because lopdf refuses to encrypt a document that already
  has an `/Encrypt` dictionary.

**Text strings and dates.** Printable ASCII is written as a literal string. Anything else (Hangul) is
written as UTF-16BE with a `FE FF` BOM, which is what `FPDF_GetMetaText` decodes. `ModDate` defaults
to the local time now, in the form `D:YYYYMMDDHHmmSS+HH'mm'`.

**XMP drop.** Both metadata paths delete the catalog `/Metadata` stream. Acrobat and most DAMs prefer
XMP over `/Info`, so a stale XMP packet would keep showing the old title after an edit, and would leak
the author after 메타데이터 제거. Regenerating XMP would need an XML writer, which is not worth it for
a light app. Every reader falls back to `/Info`.

**Encrypted documents are refused** for set/remove metadata with `unsupported`, and no undo entry is
pushed. Re-encrypting would need the owner password, which a user-password open does not give us.
The UI shows the hint and disables 적용 and 메타데이터 제거.

**Partial permissions.** `PermissionsRequest` defaults every missing flag to *allowed* and ignores
`revision`. The frontend always sends all four flags, so an explicit `false` is never lost.
`COPYABLE_FOR_ACCESSIBILITY` (bit 10) is always granted.

**Clearing a field.** `set_metadata` treats an omitted field as "keep" and a blank string as
"remove". The frontend sends a cleared field as `""`. (Fixed during verification: it used to send
`undefined`, which JSON drops, so clearing a field did nothing.)

## 3. Gates (verification run, 2026-09-23)

| gate | result |
|---|---|
| `cargo build --release --tests && cargo test --release` | 143 passed, 0 failed (incl. 10 in `tests/security.rs`) |
| `cargo check` | clean, 0 warnings |
| `npm run typecheck` | pass |
| `npx vitest run` | 38 files, 298 tests passed |
| `node scripts/check-i18n.mjs` | ok, ko 464 / en 464 keys |
| `npm run build && node scripts/check-bundle-size.mjs` | ok; critical path 105.9 kB gz of 120 kB; `SecurityDialog` 1.48 kB gz chunk |

## 4. Files changed

Backend:
* `src-tauri/Cargo.toml` adds lopdf 0.45, getrandom 0.4, and chrono with the `clock` feature.
* `src-tauri/src/engine/security.rs` (new) and `src-tauri/src/commands/security.rs`.
* `engine/registry.rs` (`mutate_bytes`, `announce`, raw permission and metadata reads).
* `engine/save/mod.rs` (`write_info`, `serialize_with`, `pdf_text_string`, `pdf_date_now`).
* `engine/raw/doc.rs` (`meta_text`), `ipc/types.rs` (`PermissionsRequest`), `engine/mod.rs`.
* `tests/security.rs` (10 tests).

Frontend:
* `src/dialogs/SecurityDialog.tsx` and `security.ts` (new), `docInfo.ts` (new), `DocInfoDialog.tsx`.
* `DialogHost.tsx`, `dialogState.ts`, `dialogs.css`, `app/useCommands.ts`, `ipc/mock.ts`, and
  `i18n/{ko,en}.json`.
* Tests: `security.test.ts`, `docInfo.test.ts`, `docInfo.flow.test.tsx`.

## 5. Still open

* `SecurityRevision` has no `r5`/`r6` value, so AES-256 files report `unknown`. Adding it needs
  `src/ipc/types.ts` and a contract edit.
* Undo labels (`undo.metadataEdit`, `undo.metadataRemove`, and every other `undo.*`) are not in
  i18n. The frontend does not show `undoLabel` today.
* ~~When 권한 암호 is left blank, the open password is used as the owner password, so the
  permission checkboxes do nothing.~~ **Resolved in review**: `validatePasswords` returns
  `ownerRequired` whenever a permission is unchecked and the owner password is blank or equal to
  the open password; the dialog shows `dialog.security.ownerRequired` and keeps 암호 설정 disabled.
  The blank-owner fallback is still allowed when nothing is restricted, where it is harmless.
* ~~Metadata edit on an encrypted document (decrypt, then re-encrypt with the same passwords) is not
  supported.~~ **Resolved in v0.3 (pkg3, S2)**: every lopdf rewrite of an encrypted document works on
  PDFium's decrypted serialisation and is re-encrypted with the file's own security-handler state
  (`engine::security::Crypt`, `IPC_CONTRACT.md` §7.11); only the modify permission refuses it.
* The two 암호 확인 inputs share one aria-label.
* For R3/R4 files, `extract_text` now reads bit 5 instead of bit 10 (ISO 32000-2 table 22). This is
  a behaviour change from Stage 2.
* No manual QA in the real app yet: check that Acrobat and Preview open the AES-256 copy, and that
  the Hangul title shows in Finder's Get Info.
