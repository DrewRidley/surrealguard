/**
 * `SurrealGuardError` wraps, it does not replace.
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
 *   if (e instanceof SurrealGuardError) {
 *     console.error(e.query, e.params);
 *     if (e.cause instanceof AuthenticationError) redirectToLogin();
 *   }
 * }
 * ```
 */

export interface SurrealGuardErrorContext {
  /** The query text that failed. */
  query?: string | undefined;
  /** The parameters it was run with. */
  params?: Record<string, unknown> | undefined;
  /** The original error, kept with its own type intact. */
  cause?: unknown;
}

export class SurrealGuardError extends Error {
  override readonly name = "SurrealGuardError";
  readonly query: string | undefined;
  readonly params: Record<string, unknown> | undefined;
  override readonly cause: unknown;

  constructor(message: string, context: SurrealGuardErrorContext = {}) {
    super(message);
    this.query = context.query;
    this.params = context.params;
    this.cause = context.cause;
  }

  /**
   * Attach query context to whatever the SDK (or the network) threw. An error
   * that is already a `SurrealGuardError` passes through unchanged, so context
   * is not layered twice as it propagates.
   */
  static from(cause: unknown, context: Omit<SurrealGuardErrorContext, "cause">): SurrealGuardError {
    if (cause instanceof SurrealGuardError) return cause;
    const detail = cause instanceof Error ? cause.message : String(cause);
    const where = context.query ? `: ${context.query}` : "";
    return new SurrealGuardError(`${detail}${where}`, { ...context, cause });
  }
}
