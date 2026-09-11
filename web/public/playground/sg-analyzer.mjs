// Loader + marshalling for the SurrealQL Analyzer engine WASM module.
//
// The module is a `wasm32-wasip1` cdylib exporting three functions
// (`sg_alloc`, `sg_dealloc`, `sg_analyze`) plus its linear `memory`. It
// imports nine `wasi_snapshot_preview1` functions; none are exercised on the
// analysis happy path except `random_get` (HashMap seeding), so a tiny
// self-contained shim is enough — no external WASI runtime, works in both the
// browser and Node.
//
// Newer modules also export `sg_analyze2`, which returns
// `{ diagnostics, statements }` — the same diagnostics plus the inferred
// response kind of each top-level statement. It is detected, never assumed:
// the `.wasm` and this `.mjs` are separate files on a CDN with separate cache
// lifetimes, so a page can genuinely load a script newer than its module.
// When the export is missing, `analyzeWithTypes` still answers with the
// diagnostics and an empty `statements`, and the caller draws no chips.

const WASI_ESUCCESS = 0;
const WASI_EBADF = 8;

function wasiShim(getMemory) {
  const view = () => new DataView(getMemory().buffer);
  const bytes = () => new Uint8Array(getMemory().buffer);
  const textDecoder = new TextDecoder();

  return {
    // Fill [ptr, ptr+len) with cryptographically-strong random bytes.
    random_get(ptr, len) {
      const buf = bytes().subarray(ptr, ptr + len);
      crypto.getRandomValues(buf);
      return WASI_ESUCCESS;
    },
    // No environment variables.
    environ_sizes_get(countPtr, sizePtr) {
      const dv = view();
      dv.setUint32(countPtr, 0, true);
      dv.setUint32(sizePtr, 0, true);
      return WASI_ESUCCESS;
    },
    environ_get() {
      return WASI_ESUCCESS;
    },
    // No preopened directories.
    fd_prestat_get() {
      return WASI_EBADF;
    },
    fd_prestat_dir_name() {
      return WASI_EBADF;
    },
    fd_close() {
      return WASI_ESUCCESS;
    },
    fd_seek() {
      return WASI_ESUCCESS;
    },
    // Route any writes (panic messages on fds 1/2) to the console.
    fd_write(fd, iovsPtr, iovsLen, nwrittenPtr) {
      const dv = view();
      const mem = bytes();
      let written = 0;
      const chunks = [];
      for (let i = 0; i < iovsLen; i++) {
        const base = dv.getUint32(iovsPtr + i * 8, true);
        const len = dv.getUint32(iovsPtr + i * 8 + 4, true);
        chunks.push(mem.subarray(base, base + len));
        written += len;
      }
      const total = chunks.reduce((n, c) => n + c.length, 0);
      const merged = new Uint8Array(total);
      let off = 0;
      for (const c of chunks) {
        merged.set(c, off);
        off += c.length;
      }
      const text = textDecoder.decode(merged);
      if (text.length) (fd === 2 ? console.error : console.log)(text);
      dv.setUint32(nwrittenPtr, written, true);
      return WASI_ESUCCESS;
    },
    // A guest abort should surface as a JS error, not a silent trap.
    proc_exit(code) {
      throw new Error(`surrealql-analyzer wasm called proc_exit(${code})`);
    },
  };
}

/**
 * Instantiate the analyzer from raw wasm bytes.
 * @param {BufferSource} wasmBytes
 * @returns {Promise<{
 *   analyze(schema: string, query: string): object[],
 *   analyzeWithTypes(schema: string, query: string): {diagnostics: object[], statements: object[]},
 *   hasTypes: boolean,
 * }>}
 */
export async function createAnalyzer(wasmBytes) {
  let instance;
  const getMemory = () => instance.exports.memory;

  const { instance: inst } = await WebAssembly.instantiate(wasmBytes, {
    wasi_snapshot_preview1: wasiShim(getMemory),
  });
  instance = inst;

  // Reactor modules run static ctors via `_initialize` when present.
  if (typeof instance.exports._initialize === "function") {
    instance.exports._initialize();
  }

  const { sg_alloc, sg_dealloc, sg_analyze, sg_analyze2, memory } = instance.exports;
  const encoder = new TextEncoder();
  const decoder = new TextDecoder();
  const hasTypes = typeof sg_analyze2 === "function";

  function writeString(str) {
    const encoded = encoder.encode(str);
    const len = encoded.length;
    if (len === 0) return { ptr: 0, len: 0 };
    const ptr = sg_alloc(len);
    new Uint8Array(memory.buffer).set(encoded, ptr);
    return { ptr, len };
  }

  /** Calls one of the `(schema, query) -> packed u64` exports and parses its JSON. */
  function call(fn, schema, query, empty) {
    const s = writeString(schema);
    const q = writeString(query);
    // u64 result: (ptr << 32) | len. The call may grow memory, so read views
    // *after* it returns.
    const packed = BigInt.asUintN(64, fn(s.ptr, s.len, q.ptr, q.len));
    const resPtr = Number(packed >> 32n);
    const resLen = Number(packed & 0xffffffffn);

    let json = empty;
    if (resLen > 0) {
      const bytes = new Uint8Array(memory.buffer, resPtr, resLen).slice();
      json = decoder.decode(bytes);
      sg_dealloc(resPtr, resLen);
    }
    if (s.len) sg_dealloc(s.ptr, s.len);
    if (q.len) sg_dealloc(q.ptr, q.len);
    return JSON.parse(json);
  }

  return {
    /** True when this module can report per-statement response kinds. */
    hasTypes,
    analyze(schema, query) {
      return call(sg_analyze, schema, query, "[]");
    },
    /**
     * Diagnostics plus one `{kind, start, end, response}` per top-level query
     * statement. On a module without `sg_analyze2` the diagnostics are still
     * real and `statements` is empty — degrade, never throw.
     */
    analyzeWithTypes(schema, query) {
      if (!hasTypes) return { diagnostics: call(sg_analyze, schema, query, "[]"), statements: [] };
      return call(sg_analyze2, schema, query, '{"diagnostics":[],"statements":[]}');
    },
  };
}
