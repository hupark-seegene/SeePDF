# Stage 10 — Microsoft Information Protection (MIP) protected PDFs

Goal (user request, 2026-10-18): *"MIP 보안이 걸린 파일도 승인된 PC라면 해당 파일을 읽을 수 있게 해줘."*
A PDF encrypted with Microsoft Purview / Azure Information Protection (sensitivity label, IRM) must open
in SeePDF when the signed-in Microsoft work account has rights to it — the same way Edge and Acrobat
open it. Today PDFium opens the ISO 32000-2 *wrapper* and shows its "This document is protected" cover
page; a legacy `.ppdf` does not open at all.

This document is the contract both implementation agents work from. Decisions here are final unless
a fact below turns out to be wrong — then fix the fact here first.

## 0. Facts established (verified 2026-10-18)

* **Format.** A MIP-protected PDF is an ISO 32000-2 "unencrypted wrapper" document: a cover page plus
  the original PDF as an embedded *encrypted payload*. Markers (pdf.js #9300, Apache Tika TIKA-4175):
  trailer `/Wrapper << /Type /MicrosoftIRMServices /Version 2 >>`, the file specification carries
  `/AFRelationship /EncryptedPayload` and `/EP << /Type /EncryptedPayload /Subtype /MicrosoftIRMServices >>`.
  The legacy container is `.ppdf` (not a PDF). The payload cipher/key wrapping is Microsoft's; only the
  **MIP SDK** decrypts it, after fetching a use license from Azure RMS with an Entra ID token.
* **MIP SDK 1.18.148** (published 2026-09-04) — File SDK, C++:
  * macOS zip: `https://download.microsoft.com/download/966278f4-ad94-4db3-8503-d5bf02738e51/mip_sdk_file_macos_1.18.148.zip`
    (1.1 GB, debug+release, arm64 **and** x86_64, `minos 11.10`). Dylibs link each other via `@rpath`.
    Release arm64 set we need ≈ 36 MB uncompressed (`libmip_file_sdk` 10.9, `libmip_core` 5.2,
    `libcrypto.3` 4.7, `libmip_protection_sdk` 3.5, `libmip_upe_sdk` 2.2 + glib/gio/gmime/gsf/… small).
  * Windows: NuGet `Microsoft.InformationProtection.File` 1.18.148
    (`https://api.nuget.org/v3-flatcontainer/microsoft.informationprotection.file/1.18.148/microsoft.informationprotection.file.1.18.148.nupkg`,
    125 MB): `build/native/{include,bins,lib}/release/{amd64,arm64,x86}` — 27 DLLs ≈ 37 MB for amd64,
    import lib `mip_file_sdk.lib`. Needs VC++ 2022 redistributable (MSVC ≥ 14.44) at runtime.
  * The **C API (`mip_cc`) covers only Protection + UPE, not the File SDK.** The File SDK is C++
    (`include/mip/file/*.h`). So we write a tiny C++ shim with a C ABI (see §2).
  * Both are already unpacked locally in `src-tauri/vendor/mip-sdk/{macos,windows}/` (gitignored):
    `include/`, `bins/release/<arch>/`, Windows `lib/release/<arch>/`, `LICENSE.txt`,
    `ThirdPartyNotice.txt`, `redist.txt` (lists the redistributable files), `version.txt`.
  * Relevant C++ API (header line numbers in the macOS include):
    `mip::FileHandler::IsProtected(path)` static (file_handler.h:221–268);
    `FileEngine::CreateFileHandlerAsync(stream, actualFilePath, isAuditDiscoveryEnabled, observer, context)`
    (file_engine.h:496/519); `FileHandler::GetProtectionHandler()`;
    `FileHandler::GetDecryptedTemporaryStreamAsync(context)` (file_handler.h:574) with
    `FileHandler::Observer::OnGetDecryptedTemporaryStreamSuccess/Failure`;
    `ProtectionHandler::GetRights()`, `AccessCheck(right)`, `GetIssuedTo()`, `GetOwner()`,
    `GetProtectionDescriptor()` → `GetName()`, `GetDescription()`, `GetContentValidUntil()`, `GetReferrer()`;
    `mip::AuthDelegate::AcquireOAuth2Token(identity, OAuth2Challenge{GetAuthority, GetResource, GetScopes, GetClaims}, token)`
    (common_types.h:336, 452–479); `mip::ConsentDelegate::GetUserConsent(url)` (common_types.h:519);
    `mip::Stream` (stream.h:46) and `mip::CreateStreamFromStdStream` (stream_utils.h);
    errors: `NoPermissionsError{GetReferrer, GetOwner}` (error.h:685), `NoAuthTokenError` (874),
    `ConsentDeniedError` (957), `NetworkError` (464).
* **Entra ID app registration is required** (learn.microsoft.com/information-protection/develop/setup-configure-mip):
  account type *this organisation only*, redirect URI type **Public client (mobile & desktop)** =
  `http://localhost`; delegated API permissions **Azure Rights Management Services → user_impersonation**
  and **Microsoft Information Protection Sync Service → UnifiedPolicy.User.Read**; then *Grant admin
  consent*. An IPIA agreement with Microsoft is only needed for public release, not internal use.
  SeePDF therefore ships **without** a client ID; the organisation's IT (or the user, if the tenant
  allows self-service app registration) enters tenant ID + client ID in 설정.
* **Repo facts.** Dynamic loading precedent: `pdfium-render` via `libloading` 0.7 (in tree). HTTP:
  `reqwest` 0.13 is in the tree through `tauri-plugin-updater` with `native-tls` + `system-proxy`
  (+ `json`, `stream`) — adding it as a direct dependency with the same features compiles nothing new.
  Also in tree: `sha2`, `base64`, `url`, `percent-encoding`, `httparse`, `security-framework` 3.7 (macOS),
  `windows` 0.61 (Windows), `getrandom`, `lopdf` 0.45, `zip` 4. `keyring` on the USTC mirror is 4.2.0
  (check its feature/crate split before using; prefer in-tree `security-framework` / `windows`
  `Win32_Security_Credentials` if that is less new code).
  `open_document` → `registry::open_named` (`src-tauri/src/engine/registry.rs:414`); open-error
  `detail` mapping in `open_error_detail` and the frontend `api.openErrorKey` (`src/ipc/api.ts:950`);
  password flow in `src/dialogs/flows.ts:124`. `DocInfo` in `src-tauri/src/ipc/types.rs:192` /
  `src/ipc/types.ts` (frozen contract: change both + `docs/IPC_CONTRACT.md`). Release assets are
  uploaded by `softprops/action-gh-release@v2` in `.github/workflows/release.yml:290`; the updater
  endpoint is `releases/latest/download/latest.json` (same base URL for the MIP runtime zip).
  Resources bundling: `src-tauri/tauri.{macos,windows}.conf.json` `bundle.resources`.

## 1. Product decisions (v1 = read-only consumption)

1. **Open + view** a protected PDF when the account has `VIEW`. Rights are enforced:
   `PRINT` → `permissions.print`, `EXTRACT` → `permissions.copy` (text copy, OCR text export),
   `permissions.modify = false` and `permissions.annotate = false` always (we cannot re-protect on save),
   `EXPORT` or `OWNER` → 다른 이름으로 저장 / export (image, text, DOCX…) of the *decrypted* content.
   `OWNER` implies every right.
2. **Decrypted bytes never touch disk** except where the user has the right and asks (export / print
   temp file). No autosave/recovery copy, no recent-file thumbnail, no spill of the decrypted document.
3. **Sign-in is interactive, in the system browser** (OAuth 2.0 authorization code + PKCE, loopback
   redirect), once; the refresh token is kept in the OS keychain / Credential Manager. 설정 shows the
   account and offers 로그아웃.
4. **The MIP runtime is an on-demand component** (≈ 15 MB download, 36–40 MB installed), not bundled —
   CLAUDE.md "keep the app light". The first protected file offers to install it. A build can still
   bundle it into `resources/mip/<triple>/` for development.
5. No labelling, no protecting, no re-saving protected files, no `.ppdf` *writing* in v1.

## 2. Architecture

```
webview ── open_document ──► commands::documents ── mip::detect(bytes) ──► Err(unsupported/"mipProtected")
        ── mip_open_document ──► commands::mip (async, NOT engine thread)
                                   │ spawn_blocking
                                   ▼
                              mip::shim (libloading → libseepdf_mip.dylib / seepdf_mip.dll)
                                   │  C ABI, auth callback ──► mip::auth (OAuth PKCE, keyring, reqwest)
                                   ▼                              │ emits `mip-auth` events, opener → browser
                              decrypted PDF bytes + rights        │ loopback 127.0.0.1:<port>
                                   │
                                   ▼
                     engine thread: registry::open_named(..., protection: Some(MipInfo))  →  DocInfo
```

### 2.1 Shim — `src-tauri/mip-shim/` (C++17, C ABI, built by a script, loaded at runtime)

`shim.h` is the ABI (version constant `SEEPDF_MIP_ABI = 1`, returned by `seepdf_mip_abi()`):

```c
typedef int (*seepdf_mip_token_cb)(void* ctx, const char* identity, const char* authority,
                                    const char* resource, const char* scopes_json,
                                    const char* claims, char** token_out /* malloc via seepdf_mip_alloc */);
typedef void (*seepdf_mip_log_cb)(void* ctx, int level, const char* msg);
int  seepdf_mip_abi(void);
int  seepdf_mip_init(const char* client_id, const char* app_name, const char* app_version,
                     const char* state_dir, seepdf_mip_log_cb log, void* log_ctx, char** err);
int  seepdf_mip_is_protected(const uint8_t* bytes, size_t len, const char* path_hint);   /* 1/0/-1 */
int  seepdf_mip_decrypt(const uint8_t* bytes, size_t len, const char* path_hint,
                        const char* identity /* signed-in UPN */, seepdf_mip_token_cb cb, void* cb_ctx,
                        uint8_t** out, size_t* out_len, char** info_json, char** err, int* err_kind);
void seepdf_mip_free(void* p);
void seepdf_mip_shutdown(void);
```

* `info_json`: `{"rights":[...],"owner":"","issuedTo":"","templateName":"","description":"","validUntil":"RFC3339|null","contentId":""}`.
* `err_kind`: 0 generic, 1 noAccess (`NoPermissionsError`; `err` = `owner|referrer`), 2 auth
  (`NoAuthTokenError`, token cb returned non-zero), 3 network, 4 consent, 5 notProtected, 6 cancelled.
* Inside: one `MipContext` (`ApplicationInfo{client_id, "SeePDF", version}`, `MipConfiguration` with
  `state_dir`, **telemetry opted out**, `LogLevel::Warning` → `log` callback), one `FileProfile`
  (`CacheStorageType::InMemory` — `OnDiskEncrypted` would hit the macOS keychain with prompts), one
  `FileEngine` per identity (re-created when the identity changes), `ConsentDelegate` returning
  `Consent::AcceptAlways`, `AuthDelegate` forwarding to `cb`. Decrypt = `CreateFileHandlerAsync`
  (input `mip::Stream` over a `std::stringstream` of `bytes`, `path_hint` for type detection) →
  `GetProtectionHandler()` (null → kind 5) → `AccessCheck("VIEW")` → `GetDecryptedTemporaryStreamAsync`
  into a memory stream → copy to `out`. All async calls are awaited with promise/future (the shim is
  only ever called from a blocking worker thread, never the engine thread or the UI).
  `seepdf_mip_shutdown` calls `MipContext::ShutDown()`; the app calls it on exit (the SDK can hang or
  crash the process otherwise).
* Build: `scripts/build-mip-shim.mjs` — macOS `clang++ -std=c++17 -O2 -fvisibility=hidden -shared
  -I vendor/mip-sdk/macos/include -L bins/release/<arch> -lmip_file_sdk -Wl,-rpath,@loader_path
  -install_name @rpath/libseepdf_mip.dylib` (`-arch arm64` / `-arch x86_64`); Windows
  `cl /std:c++17 /O2 /EHsc /MD /LD shim.cpp mip_file_sdk.lib` through `vcvarsall` (the CI windows
  runner has MSVC; locally on this Mac only the macOS build is possible). Output + the runtime libs
  (macOS: every `@rpath/` dylib `otool -L` lists transitively, Windows: `redist.txt`'s DLL list — check
  the exact names on disk, e.g. `libxml2-*.dll`) + `LICENSE.txt`/`ThirdPartyNotice.txt` + a
  `manifest.json` `{sdkVersion, abi, triple, files:[{name,sha256,size}]}` → `src-tauri/resources/mip/<triple>/`
  (gitignored, add to `.gitignore`).
* `scripts/fetch-mip.mjs` downloads/unpacks the SDK into `src-tauri/vendor/mip-sdk/` (skips when
  `version.txt` matches; version + URLs + sha256 in `scripts/mip-sdk.json`). `scripts/pack-mip-runtime.mjs`
  zips `resources/mip/<triple>/` → `seepdf-mip-runtime-<triple>.zip` + `seepdf-mip-runtime-<triple>.sha256`.
  Add `npm run mip:fetch`, `mip:build`, `mip:pack`.
* CI: a `mip-runtime` job per release target (macOS arm64 + x86_64 — both can be compiled on the arm64
  runner with `-arch`; Windows x64; Windows arm64 only if the release matrix already builds it) that
  caches the SDK download (`actions/cache` keyed by `scripts/mip-sdk.json`) and adds the two files to
  the release assets. `ci.yml` builds the shim on macOS and runs the smoke test (§5) so a broken shim is
  caught before release.

### 2.2 Rust — `src-tauri/src/mip/` (not under `engine/`: no pdfium here)

* `detect.rs` — `pub fn detect(bytes: &[u8], path: Option<&Path>) -> Option<MipKind>` (`Wrapper`,
  `Legacy`). Pure: `.ppdf` extension → `Legacy`; a `%PDF-` file containing `/MicrosoftIRMServices` or
  `/AFRelationship /EncryptedPayload` (byte scan, whole file, `memchr`-style — the names are plaintext
  in the wrapper) → `Wrapper`. Unit tests with a synthetic wrapper built in-test with `lopdf` (cover
  page + `/EmbeddedFiles` file spec with `/AFRelationship /EncryptedPayload` and
  `/EP << /Type /EncryptedPayload /Subtype /MicrosoftIRMServices /Version 2 >>`, trailer `/Wrapper`),
  written to a temp dir, never into `fixtures/`.
* `runtime.rs` — locating and installing the component. Lookup order: `<resource_dir>/mip/<triple>/`
  (dev / bundled) → `<app_data_dir>/mip/<triple>/<sdkVersion>/`. `status()`, `install(app, on_progress)`
  downloads `<release base>/seepdf-mip-runtime-<triple>.zip` + `.sha256` with `reqwest`
  (system proxy, like the updater), verifies, extracts with `zip`, validates `manifest.json`, atomically
  renames into place. `remove()`.
* `shim.rs` — `libloading` bindings to the ABI above; `Mip` is a process-wide `OnceLock<Mutex<_>>`;
  loads lazily; on Windows load with the runtime dir as DLL search dir (libloading's default
  `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR` flags do this for a full path). `init` once per process with the
  configured client ID (re-init is a restart; the SDK does not support changing the app ID — say so in
  설정).
* `auth.rs` — OAuth 2.0 authorization code + PKCE (S256) public client, loopback redirect
  `http://localhost:<port>` (random free port on 127.0.0.1; Entra accepts any port for a registered
  `http://localhost`). Endpoints `https://login.microsoftonline.com/<tenant>/oauth2/v2.0/{authorize,token}`
  — the SDK's challenge authority is used when it names a tenant (`https://login.windows.net/<tid>` /
  `login.microsoftonline.com/<tid>`), otherwise the configured tenant. Scopes: the challenge's scopes when
  non-empty, else `<resource>/.default`; always add `offline_access openid profile email`. Pass
  `claims` through when the challenge carries one (Conditional Access). The browser is opened with
  `tauri-plugin-opener`; the loopback replies with a small bilingual (ko/en) HTML "로그인이 완료되었습니다 —
  SeePDF로 돌아가세요". Timeout 5 min; `cancel()` closes the listener. Token exchange with `reqwest`
  (form-encoded). The **refresh token** is the only thing persisted — OS keychain (macOS
  `security-framework` generic password, service `com.seepdf.desktop.mip`, account `<tenant>|<clientId>`;
  Windows `CredWriteW` generic credential, same name) — access tokens live in memory with expiry.
  Refresh (`grant_type=refresh_token`, with the resource's scope) before falling back to interactive.
  `sign_in()` performs an interactive flow for `https://aadrm.com/.default` and records the account
  (`preferred_username` / `name` from the id_token, no verification needed — display only).
  `sign_out()` deletes the keychain item and the in-memory cache. Config `{tenantId, clientId}` in
  `<app_config_dir>/mip.json` (default `tenantId = "organizations"`, `clientId` from
  `option_env!("SEEPDF_MIP_CLIENT_ID")` or empty).
* `mod.rs` — `open(app, path, bytes) -> Result<Decrypted{bytes, info: MipInfo}, MipError>` tying it
  together, emitting `mip-auth` events `{phase: "signingIn" | "decrypting" | "installing"}` to the
  calling window. `MipError → EngineError`: runtime missing → `unsupported` + detail `mipRuntimeMissing`;
  no client ID → `unsupported` + `mipNotConfigured`; kind 1 → `permissionDenied` + `mipNoAccess`
  (message = owner email, referrer URL in `message` after `|`); kind 2 → `permissionDenied` + `mipAuthFailed`;
  kind 3 → `io` + `mipNetwork`; cancelled → `cancelled`; everything else → `pdfium`/`io` + `mipFailed`
  with the SDK message.

### 2.3 Engine / IPC changes

* `open_document`: after reading the bytes and **before** pdfium, `mip::detect` → `Err(unsupported,
  detail "mipProtected")`. (The wrapper would otherwise open as a one-page cover document.)
* `registry::open_named` gains `protection: Option<ProtectionInfo>` (add a sibling
  `open_protected(...)` rather than touching every caller). With `Some`: `encrypted = true` (so every
  lopdf-based mutation already refuses), `permissions` as §1, `OpenDoc.protection` stored.
  Refusals (with `detail`): `save_document` → `unsupported`/`mipReadOnly`; `save_as`, every export
  command, print temp-file creation → `permissionDenied`/`mipExportDenied` unless `EXPORT`/`OWNER`
  (print: `PRINT`); autosave/recovery (`recovery.rs`) and `write_recent_thumbnail` skip protected docs;
  `history` spill to disk must not happen (no edits are possible — assert `modify=false` keeps it so).
* `DocInfo.protection?: ProtectionInfo` — `{ kind: "mip", rights: string[], owner: string, issuedTo: string,
  templateName: string, description: string, validUntil: string | null, account: string }` (absent for
  ordinary documents; `#[serde(skip_serializing_if = "Option::is_none")]`). Update `types.rs`,
  `types.ts`, `docs/IPC_CONTRACT.md`.
* New commands (`src-tauri/src/commands/mip.rs`, registered in `lib.rs` + capabilities):
  `mip_open_document(path, display_name?) -> DocInfo`, `mip_cancel_auth()`,
  `mip_runtime_status() -> { installed: bool, version: string | null, triple: string, downloadUrl: string }`,
  `mip_runtime_install() -> JobId` (progress through the existing job/progress event mechanism — follow
  how `compress`/`batchOcr` report progress), `mip_runtime_remove()`,
  `mip_get_config() -> { tenantId, clientId, configured: bool }`, `mip_set_config({tenantId, clientId})`,
  `mip_get_account() -> { signedIn: bool, username: string | null }`, `mip_sign_in() -> account`,
  `mip_sign_out()`.

### 2.4 Frontend (`src/`)

* `src/mip/flow.ts` — `openProtected(path, displayName?)`: `mip_runtime_status` → not installed → dialog
  `mip.install.title/body` (size, what it is, Microsoft licence note) with 설치 / 취소 → install job in
  the status bar → then `mip_get_config` → not configured → dialog `mip.setup.title/body` with 설정 열기 /
  취소 (the body says IT must register the app and names the two permissions) → `mip_open_document`
  while a non-blocking sheet shows `mip.signingIn` / `mip.decrypting` (from `mip-auth` events) with 취소
  (`mip_cancel_auth`). Hook it into `src/dialogs/flows.ts` `openPath` (and the batch / multi-search /
  compare flows: a protected file there is *skipped* with reason `mip.skipped`, like an encrypted one
  whose password was dismissed).
* Errors: `api.openErrorKey` → `error.mipRuntimeMissing`, `error.mipNotConfigured`, `error.mipNoAccess`
  (toast shows the owner from `message`: "문서 소유자 {owner}에게 권한을 요청하세요"), `error.mipAuthFailed`,
  `error.mipNetwork`, `error.mipFailed`, `error.mipReadOnly`, `error.mipExportDenied`.
* Status bar badge `mip.badge` ("보호됨") next to 암호화됨 / 읽기 전용; popover: 라벨 이름 + 설명, 소유자,
  계정, 만료, 내 권한 as chips (보기 · 인쇄 · 복사 · 내보내기 · 편집, unavailable ones muted) and the
  explanation `mip.readOnlyNote` ("보호된 문서는 SeePDF에서 편집하거나 저장할 수 없습니다").
* Gating: `protection` present → 저장 hidden/disabled (`error.mipReadOnly` tooltip); 다른 이름으로 저장 /
  내보내기 / OCR 텍스트 저장 disabled unless `EXPORT`/`OWNER`; print follows `permissions.print` as today;
  copy follows `permissions.copy` as today. Recents: entry kept, no thumbnail.
* 설정 → new section `mip.settings.title` ("Microsoft 보호 문서 (MIP)"): 테넌트 ID, 클라이언트 ID
  (help text + "IT 관리자에게 전달할 안내 보기" that opens `docs/STAGE10_MIP_NOTES.md`'s setup section
  rendered as a small read-only dialog or the GitHub docs URL via opener), 계정 (로그인 / 로그아웃 +
  username), 구성 요소 (설치됨 vN / 설치되지 않음, 설치 / 제거). Changing the client ID after the SDK was
  initialised shows `mip.settings.restartNote`.
* i18n: every string in `ko.json` **and** `en.json`; `npm run check:i18n` must pass.
* Tests (vitest, following `src/app/security.flow.test.tsx` conventions): open flow (runtime missing →
  install → configured → sign-in sheet → `DocInfo.protection` → badge + disabled 저장), `mipNoAccess`
  toast with owner, settings section (config round-trip, sign in/out), export gating, `openErrorKey` map.

## 3. Setup documentation (for the user's IT) — `docs/STAGE10_MIP_NOTES.md`

Must contain, in Korean with the exact English UI names of the Azure portal: Entra → 앱 등록 → 새 등록;
지원되는 계정 유형: 이 조직 디렉터리의 계정만; 리디렉션 URI: **공용 클라이언트/네이티브(모바일 및 데스크톱)**
`http://localhost`; API 권한 추가: *Azure Rights Management Services* → 위임된 권한 → `user_impersonation`;
*내 조직에서 사용하는 API* → *Microsoft Information Protection Sync Service* → `UnifiedPolicy.User.Read`;
*관리자 동의 허용*; copy 애플리케이션(클라이언트) ID + 디렉터리(테넌트) ID into SeePDF 설정. Plus: IPIA
not needed for internal use; the runtime component is Microsoft's redistributable MIP SDK
(`redist.txt`, `LICENSE.txt`); security model (§1 2.); what is logged; proxy note (SDK uses the
system proxy; the loopback redirect is local only); macOS notarisation note — when the app is signed
with hardened runtime, the downloaded dylibs need `com.apple.security.cs.disable-library-validation`
or must be signed by the same team in CI (add the entitlement now if an entitlements file exists,
otherwise document it); Windows needs the VC++ 2022 redistributable (usually present; the install
dialog links to it when `seepdf_mip.dll` fails to load with `ERROR_MOD_NOT_FOUND`).

## 4. Out of scope (backlog)

Labelling / protecting / re-saving protected PDFs, `.ppdf` writing, use-license offline cache,
WAM/broker single sign-on on Windows, `HttpDelegate` routing the SDK's HTTP through reqwest, Linux.

## 5. Verification before hand-off

* `cargo test` (detection, PKCE/URL/loopback parsing, rights → permissions mapping, `open_document`
  refusing a wrapper with `mipProtected`, export/save refusals on a protected `OpenDoc`), `cargo clippy`.
* Shim built locally for `aarch64-apple-darwin`; `cargo run --example mip_smoke -- <pdf>` loads the
  shim from `resources/mip/<triple>/`, prints `abi`, `init` result (client ID may be a dummy GUID) and
  `is_protected` for the synthetic wrapper (expect 1) and `fixtures/tracemonkey.pdf` (expect 0) —
  no network, no account.
* `npm run typecheck`, `npx vitest run`, `npm run check:i18n`, `npm run build` + bundle-size gate.
* Real decrypt needs a tenant + a protected sample: **owed by the user** (QA checklist at the end of
  the notes). Nothing in CI or tests may call Microsoft services.
