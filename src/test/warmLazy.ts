/**
 * Flow tests that render the whole `<App />` reach a dozen `lazy(() => import(…))` chunks (the
 * welcome screen, the dialog host, the annotation host, the 주석 list, toasts, the context
 * menu…). The first test of a file used to pay for transforming and evaluating every one of them
 * inside its own 5 s budget — ~1.8 s of a 2.1 s test when idle, and over 5 s when the machine was
 * busy (a parallel `cargo build`), which is what made those tests flaky.
 *
 * `beforeAll(() => warmLazyChunks(…))` loads them once per file under the hook timeout instead,
 * so a test's clock measures the flow, not the module graph. Pass the file's own extra chunks
 * (a dialog it opens) as loaders.
 */
export function warmLazyChunks(...extra: (() => Promise<unknown>)[]): Promise<unknown> {
  return Promise.all([
    import("../welcome"),
    import("../dialogs/DialogHost"),
    import("../app/Toasts"),
    import("../app/ContextMenu"),
    import("../app/Inspector/InspectorBody"),
    import("../sidebar/AnnotationList"),
    import("../annot/AnnotationHost"),
    ...extra.map((load) => load()),
  ]);
}
