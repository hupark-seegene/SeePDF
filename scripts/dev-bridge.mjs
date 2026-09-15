/**
 * Dev-only command bridge for the Stage 2 end-to-end smoke (`docs/STAGE2_INTEGRATION.md`).
 *
 * It exists **only inside the Vite dev server** — `npm run build` never sees it, the Tauri
 * bundle never serves it, and the browser half (`src/dev/testHook.ts`) is behind
 * `import.meta.env.DEV`. It lets a shell drive the real app without a devtools console:
 *
 *   curl -s -XPOST localhost:1420/__dev/cmd -d '{"op":"state"}'
 *   curl -s -XPOST localhost:1420/__dev/cmd -d '{"op":"run","id":"file.save"}'
 *
 * The POST blocks until the page has answered (or 30 s pass), so one curl call is one step.
 */
export function devBridge() {
  /** @type {{ id: number; body: unknown }[]} */
  let queue = [];
  /** @type {Map<number, (value: unknown) => void>} */
  const waiting = new Map();
  let nextId = 1;

  return {
    name: "seepdf-dev-bridge",
    apply: /** @type {const} */ ("serve"),
    configureServer(server) {
      server.middlewares.use("/__dev/cmd", (req, res) => {
        if (req.method === "GET") {
          const batch = queue;
          queue = [];
          res.setHeader("content-type", "application/json");
          res.end(JSON.stringify(batch));
          return;
        }
        if (req.method !== "POST") {
          res.statusCode = 405;
          res.end();
          return;
        }
        let raw = "";
        req.on("data", (c) => (raw += c));
        req.on("end", () => {
          let body;
          try {
            body = JSON.parse(raw || "{}");
          } catch (e) {
            res.statusCode = 400;
            res.end(String(e));
            return;
          }
          const id = nextId++;
          queue.push({ id, body });
          const timer = setTimeout(() => {
            waiting.delete(id);
            res.setHeader("content-type", "application/json");
            res.end(JSON.stringify({ ok: false, error: "timeout: the page did not answer" }));
          }, 30_000);
          waiting.set(id, (value) => {
            clearTimeout(timer);
            res.setHeader("content-type", "application/json");
            res.end(JSON.stringify(value));
          });
        });
      });

      server.middlewares.use("/__dev/result", (req, res) => {
        let raw = "";
        req.on("data", (c) => (raw += c));
        req.on("end", () => {
          try {
            const { id, ...rest } = JSON.parse(raw || "{}");
            waiting.get(id)?.(rest);
            waiting.delete(id);
          } catch {
            /* ignore */
          }
          res.statusCode = 204;
          res.end();
        });
      });
    },
  };
}
