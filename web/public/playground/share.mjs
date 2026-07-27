// The playground's state, in the URL.
//
// A playground's social function is the link: you paste a URL and the person
// who opens it sees *exactly* what you were looking at. Everything the page
// shows is derived from two strings — the schema and the query — so those two
// strings are the whole of the shareable state, and they live in the hash.
//
// The hash, not the query string, because a hash never reaches the server:
// nothing anyone types here is logged by the CDN, which is the same promise the
// page makes about the analyzer running locally.
//
// Three forms, distinguished by a one-character tag before the first `/`:
//
//   #x/<slug>    one of the built-in examples, by name. Short and legible, so
//                the link you get from picking "Graph edge" reads as
//                `#x/graph-edge` rather than 400 characters of base64. It also
//                keeps working if a preset's text is edited later.
//   #c/<data>    schema + query, DEFLATE'd, base64url. The normal case.
//   #p/<data>    the same pair with no compression, for a browser without
//                `CompressionStream` (Safari < 16.4). Written only as a
//                fallback; always *read*, so old links keep resolving.
//
// The pair is joined with a NUL — a byte that cannot occur in SurrealQL source
// — rather than wrapped in JSON, which would spend a fifth of the budget on
// escaping quotes and newlines.

const SEP = "\u0000";
const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** Anything past this and we stop rewriting the URL — see `encodeState`. */
export const MAX_SHARE_BYTES = 64 * 1024;

/** `Unknown field` -> `unknown-field`. Stable enough to put in a URL. */
export const slugify = (label) =>
  String(label)
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-|-$/g, "");

// --- base64url --------------------------------------------------------------

function toBase64Url(bytes) {
  let binary = "";
  // Chunked: `String.fromCharCode(...bytes)` blows the argument limit somewhere
  // around 100 KB, which is inside the range a paste can reach.
  for (let i = 0; i < bytes.length; i += 0x8000) {
    binary += String.fromCharCode.apply(null, bytes.subarray(i, i + 0x8000));
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function fromBase64Url(text) {
  const padded = text.replace(/-/g, "+").replace(/_/g, "/");
  const binary = atob(padded + "=".repeat((4 - (padded.length % 4)) % 4));
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) bytes[i] = binary.charCodeAt(i);
  return bytes;
}

// --- deflate ----------------------------------------------------------------

const hasCompression =
  typeof CompressionStream === "function" && typeof DecompressionStream === "function";

async function pipe(bytes, stream) {
  const response = new Response(new Blob([bytes]).stream().pipeThrough(stream));
  return new Uint8Array(await response.arrayBuffer());
}

const deflate = (bytes) => pipe(bytes, new CompressionStream("deflate-raw"));
const inflate = (bytes) => pipe(bytes, new DecompressionStream("deflate-raw"));

// --- the two directions -----------------------------------------------------

/**
 * The hash for a schema/query pair, `#` included.
 *
 * @param {string} schema
 * @param {string} query
 * @returns {Promise<string|null>} null when the pair is too big to put in a URL
 */
export async function encodeState(schema, query) {
  const bytes = encoder.encode(`${schema}${SEP}${query}`);
  // A URL that no chat client will carry is worse than no URL: past this the
  // page stops rewriting the address bar and says so, rather than silently
  // producing a link that arrives truncated.
  if (bytes.length > MAX_SHARE_BYTES) return null;
  if (!hasCompression) return `#p/${toBase64Url(bytes)}`;
  try {
    return `#c/${toBase64Url(await deflate(bytes))}`;
  } catch {
    return `#p/${toBase64Url(bytes)}`;
  }
}

/** The hash naming a built-in example. */
export const encodeExample = (label) => `#x/${slugify(label)}`;

/**
 * Reads a hash back.
 *
 * @param {string} hash `location.hash`, with or without the leading `#`
 * @returns {Promise<{kind: "example", slug: string}
 *                  |{kind: "source", schema: string, query: string}
 *                  |null>} null for an empty or unreadable hash
 */
export async function decodeState(hash) {
  const raw = String(hash || "").replace(/^#/, "");
  if (!raw) return null;
  const slash = raw.indexOf("/");
  if (slash < 0) return null;
  const tag = raw.slice(0, slash);
  const data = raw.slice(slash + 1);
  if (!data) return null;

  if (tag === "x") return { kind: "example", slug: data };
  if (tag !== "c" && tag !== "p") return null;

  try {
    const bytes = fromBase64Url(data);
    const text = decoder.decode(tag === "c" ? await inflate(bytes) : bytes);
    const cut = text.indexOf(SEP);
    // A payload without the separator is not one of ours.
    if (cut < 0) return null;
    return { kind: "source", schema: text.slice(0, cut), query: text.slice(cut + 1) };
  } catch {
    // A hand-mangled link, or `#c/…` in a browser that cannot inflate. Falling
    // back to the default example beats an error page over a typo in a URL.
    return null;
  }
}
