// Copies the analyzer WASM into the package.
//
// The artifact is built once, by `crates/wasm/build-wasm.sh`, and already
// lives in `web/public/playground/` for the browser playground. This package
// ships the *same bytes* rather than a second build: one module means the
// playground, the editor plugin and anything else that embeds the analyzer can
// never disagree about what a query means, and there is one place to rebuild
// when the analyzer changes.
//
// The copy is not committed. An 8 MB binary in git twice is 8 MB too many, and
// the copy is reproducible from a file that *is* committed. This script runs
// from `prepare` — which pnpm runs on install and npm runs before packing, so
// neither a fresh clone nor a publish can end up without it — and from `build`
// and `pretest`, so neither can run against a stale one.

import { copyFileSync, mkdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const packageRoot = join(here, "..");
const repoRoot = join(packageRoot, "..", "..");

const source = join(repoRoot, "web", "public", "playground", "surrealql_analyzer_wasm.wasm");
const destination = join(packageRoot, "wasm", "surrealql-analyzer.wasm");

try {
  statSync(source);
} catch {
  console.error(
    `sync-wasm: ${source} is missing.\n` +
      "Build it with ./crates/wasm/build-wasm.sh (see crates/wasm/README.md).",
  );
  process.exit(1);
}

mkdirSync(dirname(destination), { recursive: true });
copyFileSync(source, destination);
const size = (statSync(destination).size / 1024 / 1024).toFixed(1);
console.log(`sync-wasm: wrote wasm/surrealql-analyzer.wasm (${size} MB)`);
