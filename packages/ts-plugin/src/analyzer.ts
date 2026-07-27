/**
 * The SurrealGuard analyzer, loaded into the tsserver process as WebAssembly.
 *
 * The alternative was shelling out to `surrealguard check`. tsserver asks for
 * diagnostics on a keystroke cadence, and a subprocess per keystroke means a
 * process spawn, a pipe, a JSON round-trip and a schema re-read every time —
 * tens of milliseconds where the whole budget is a few. In-process WASM makes
 * a call a function call, keeps the parsed schema catalog warm between
 * keystrokes, and makes the plugin a plain `npm install` with no binary to
 * find on `$PATH` and no version skew between the CLI and the editor.
 *
 * The module is instantiated **synchronously** — `new WebAssembly.Module` on
 * bytes already in memory — because every language-service method it serves is
 * synchronous. There is no point in the call sequence where the plugin could
 * await anything.
 */

import { webcrypto } from "node:crypto";
import { readFileSync } from "node:fs";

/** What the analyzer is asked about one host file. */
export interface HostRequest {
  /** Every `.surql` source in the project, concatenated, schema first. */
  schema: string;
  /** The host file's name; only its extension is read. */
  file_name: string;
  /** The host file's full current text. */
  source: string;
  /** A UTF-8 byte offset to answer a hover for. */
  hover?: number;
}

/** One finding, at host-file **UTF-8 byte** offsets. */
export interface HostDiagnostic {
  code: string;
  severity: "error" | "warning" | "hint";
  message: string;
  start: number;
  end: number;
}

/** One highlighting token, at host-file UTF-8 byte offsets. */
export interface HostToken {
  start: number;
  end: number;
  /** Index into the shared token legend; see `classify.ts`. */
  kind: number;
}

/** One embedded query's extent, at host-file UTF-8 byte offsets. */
export interface HostQuery {
  start: number;
  end: number;
}

/** The type under the cursor, at host-file UTF-8 byte offsets. */
export interface HostHover {
  markdown: string;
  start: number;
  end: number;
}

/** Everything the analyzer knows about one host file. */
export interface HostResponse {
  diagnostics: HostDiagnostic[];
  tokens: HostToken[];
  queries: HostQuery[];
  hover: HostHover | null;
}

/** The analyzer, ready to answer. */
export interface Analyzer {
  analyzeHost(request: HostRequest): HostResponse;
}

const EMPTY: HostResponse = {
  diagnostics: [],
  tokens: [],
  queries: [],
  hover: null,
};

const WASI_ESUCCESS = 0;
const WASI_EBADF = 8;

interface Exports {
  memory: WebAssembly.Memory;
  sg_alloc(len: number): number;
  sg_dealloc(ptr: number, len: number): void;
  sg_host(ptr: number, len: number): bigint;
  _initialize?: () => void;
}

/**
 * The nine `wasi_snapshot_preview1` imports the module declares.
 *
 * Only `random_get` (HashMap seeding) is exercised on the analysis path, so a
 * ~40-line shim is enough; pulling in a real WASI runtime would add a
 * dependency to a package whose whole appeal is having none.
 */
function wasiShim(getMemory: () => WebAssembly.Memory, log: (message: string) => void) {
  const view = () => new DataView(getMemory().buffer);
  const bytes = () => new Uint8Array(getMemory().buffer);
  const decoder = new TextDecoder();

  return {
    random_get(ptr: number, len: number) {
      webcrypto.getRandomValues(bytes().subarray(ptr, ptr + len));
      return WASI_ESUCCESS;
    },
    environ_sizes_get(countPtr: number, sizePtr: number) {
      const dv = view();
      dv.setUint32(countPtr, 0, true);
      dv.setUint32(sizePtr, 0, true);
      return WASI_ESUCCESS;
    },
    environ_get: () => WASI_ESUCCESS,
    fd_prestat_get: () => WASI_EBADF,
    fd_prestat_dir_name: () => WASI_EBADF,
    fd_close: () => WASI_ESUCCESS,
    fd_seek: () => WASI_ESUCCESS,
    /**
     * A guest write is a panic message. It goes to the plugin's log, never to
     * stdout: tsserver's stdout is the protocol channel, and a stray line on it
     * desynchronises the editor's connection.
     */
    fd_write(fd: number, iovsPtr: number, iovsLen: number, writtenPtr: number) {
      const dv = view();
      const mem = bytes();
      const chunks: Uint8Array[] = [];
      let written = 0;
      for (let i = 0; i < iovsLen; i++) {
        const base = dv.getUint32(iovsPtr + i * 8, true);
        const len = dv.getUint32(iovsPtr + i * 8 + 4, true);
        chunks.push(mem.subarray(base, base + len));
        written += len;
      }
      const merged = new Uint8Array(written);
      let offset = 0;
      for (const chunk of chunks) {
        merged.set(chunk, offset);
        offset += chunk.length;
      }
      const text = decoder.decode(merged).trimEnd();
      if (text) log(`[wasm fd${fd}] ${text}`);
      dv.setUint32(writtenPtr, written, true);
      return WASI_ESUCCESS;
    },
    proc_exit(code: number) {
      throw new Error(`surrealguard wasm called proc_exit(${code})`);
    },
  };
}

/**
 * Compiles and instantiates the analyzer from `wasmPath`.
 *
 * Throws when the file is missing or the module will not instantiate. The
 * caller decides what that means — for the plugin it means staying silent, not
 * putting its own installation problem in someone's TypeScript file.
 */
export function loadAnalyzer(wasmPath: string, log: (message: string) => void = () => {}): Analyzer {
  const module = new WebAssembly.Module(readFileSync(wasmPath));
  let instance: WebAssembly.Instance;
  const getMemory = () => (instance.exports as unknown as Exports).memory;

  instance = new WebAssembly.Instance(module, {
    wasi_snapshot_preview1: wasiShim(getMemory, log),
  });
  const exports = instance.exports as unknown as Exports;
  // A reactor module runs its static constructors here.
  exports._initialize?.();

  if (typeof exports.sg_host !== "function") {
    throw new Error(
      `${wasmPath} has no sg_host export — it predates the language-service plugin`,
    );
  }

  const encoder = new TextEncoder();
  const decoder = new TextDecoder();

  return {
    analyzeHost(request: HostRequest): HostResponse {
      const encoded = encoder.encode(JSON.stringify(request));
      const ptr = exports.sg_alloc(encoded.length);
      new Uint8Array(exports.memory.buffer).set(encoded, ptr);

      // The result packs `(ptr << 32) | len`. The call can grow linear memory,
      // so every view is taken *after* it returns.
      const packed = BigInt.asUintN(64, exports.sg_host(ptr, encoded.length));
      const resultPtr = Number(packed >> 32n);
      const resultLen = Number(packed & 0xffffffffn);

      let json = "";
      if (resultLen > 0) {
        json = decoder.decode(
          new Uint8Array(exports.memory.buffer, resultPtr, resultLen).slice(),
        );
        exports.sg_dealloc(resultPtr, resultLen);
      }
      exports.sg_dealloc(ptr, encoded.length);

      if (!json) return EMPTY;
      return JSON.parse(json) as HostResponse;
    },
  };
}
