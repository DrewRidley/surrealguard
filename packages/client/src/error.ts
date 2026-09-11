/**
 * `SurrealQLAnalyzerError` wraps, it does not replace.
 *
 * The SDK ships a real error hierarchy — `SurrealError → ServerError →
 * { QueryError, ValidationError, AuthenticationError, … }`, plus a separate
 * `SqonError` family for value parsing. Flattening that would throw away
 * information callers already depend on, so the original is kept verbatim as
 * `cause` and only the missing context is added: which query failed, and with
 * what parameters.
 *
 * ```ts
 * try {
 *   await db.run(peopleOf, { team });
 * } catch (e) {
 *   if (e instanceof SurrealQLAnalyzerError) {
 *     console.error(e.query, e.params);
 *     if (e.cause instanceof AuthenticationError) redirectToLogin();
 *   }
 * }
 * ```
 */

export interface SurrealQLAnalyzerErrorContext {
  /** The query text that failed. */
  query?: string | undefined;
  /** The parameters it was run with. */
  params?: Record<string, unknown> | undefined;
  /** The original error, kept with its own type intact. */
  cause?: unknown;
}

export class SurrealQLAnalyzerError extends Error {
  override readonly name = "SurrealQLAnalyzerError";
  readonly query: string | undefined;
  readonly params: Record<string, unknown> | undefined;
  override readonly cause: unknown;

  constructor(message: string, context: SurrealQLAnalyzerErrorContext = {}) {
    super(message);
    this.query = context.query;
    this.params = context.params;
    this.cause = context.cause;
  }

  /**
   * Attach query context to whatever the SDK (or the network) threw. An error
   * that is already a `SurrealQLAnalyzerError` passes through unchanged, so context
   * is not layered twice as it propagates.
   */
  static from(cause: unknown, context: Omit<SurrealQLAnalyzerErrorContext, "cause">): SurrealQLAnalyzerError {
    if (cause instanceof SurrealQLAnalyzerError) return cause;
    const detail = cause instanceof Error ? cause.message : String(cause);
    const where = context.query ? `: ${context.query}` : "";
    return new SurrealQLAnalyzerError(`${detail}${where}`, { ...context, cause });
  }
}
