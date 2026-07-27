/**
 * UTF-8 byte offsets to UTF-16 code-unit offsets.
 *
 * The analyzer counts bytes (Rust strings are UTF-8); TypeScript counts UTF-16
 * code units (JavaScript strings are UTF-16). For ASCII the two are the same
 * number, which is why this is easy to forget and why forgetting it is
 * invisible until someone writes a comment with an accent in it — after which
 * every span later on that line lands a byte or two to the right of the token
 * it is about.
 *
 * The common case is settled in one comparison: a string whose UTF-8 length
 * equals its UTF-16 length is all-ASCII, and the conversion is the identity.
 * Otherwise only the characters that are *wider in bytes than in units* need
 * recording — a table of their positions, binary-searched — so the cost is
 * proportional to how much non-ASCII the file holds, not to how big it is.
 */

/** Converts a UTF-8 byte offset in some fixed text to a UTF-16 offset. */
export type ByteToUtf16 = (byteOffset: number) => number;

/** Converts a UTF-16 code-unit offset in some fixed text to a byte offset. */
export type Utf16ToByte = (utf16Offset: number) => number;

/**
 * Builds the reverse converter for `text` — what a cursor position needs, since
 * the editor names positions in units and the analyzer answers about bytes.
 */
export function utf16ToByte(text: string): Utf16ToByte {
  if (Buffer.byteLength(text, "utf8") === text.length) return (offset) => offset;
  return (offset) => Buffer.byteLength(text.slice(0, offset), "utf8");
}

/** One character that occupies more UTF-8 bytes than UTF-16 units. */
interface Wide {
  /** Its first byte's offset. */
  byte: number;
  /** Its first code unit's offset. */
  unit: number;
  /** How many bytes it occupies. */
  bytes: number;
  /** How many code units it occupies. */
  units: number;
}

/** Builds the converter for `text`. */
export function byteToUtf16(text: string): ByteToUtf16 {
  if (Buffer.byteLength(text, "utf8") === text.length) return (offset) => offset;

  const wide: Wide[] = [];
  let byte = 0;
  for (let unit = 0; unit < text.length; ) {
    const code = text.codePointAt(unit) ?? 0;
    const units = code > 0xffff ? 2 : 1;
    const bytes = code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
    if (bytes !== units) wide.push({ byte, unit, bytes, units });
    byte += bytes;
    unit += units;
  }

  return (offset) => {
    // The last wide character starting at or before `offset` carries all the
    // drift accumulated up to it, in its own `unit`.
    let low = 0;
    let high = wide.length - 1;
    let found: Wide | undefined;
    while (low <= high) {
      const mid = (low + high) >> 1;
      const candidate = wide[mid] as Wide;
      if (candidate.byte <= offset) {
        found = candidate;
        low = mid + 1;
      } else {
        high = mid - 1;
      }
    }
    if (!found) return offset;
    // Inside that character: the only honest answer is its own start.
    if (offset < found.byte + found.bytes) return found.unit;
    // Past it: every byte since is one unit, because any wide character in
    // between would have been the one found.
    return found.unit + found.units + (offset - found.byte - found.bytes);
  };
}
