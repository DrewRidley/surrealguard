// Who the browser is talking to the database as — and what has to happen to
// the cache when that changes.
//
// The authentication itself is SurrealDB's, not ours: `DEFINE ACCESS staff ON
// DATABASE TYPE RECORD` in `schema/schema.surql` owns the SIGNIN query, and
// `db.signin` runs it. Whatever record it returns becomes `$auth` for the
// session, and `PERMISSIONS FOR select WHERE team = $auth.team` on `ticket`
// does the rest. There is no authorisation logic in this file, or anywhere else
// in the app, which is the point of the beat.
//
// ---------------------------------------------------------------------------
// The part that is NOT decoration: `getQueryClient(db).reset()`.
//
// A cache entry is keyed by query text plus parameters. `$auth` is in neither.
// So `SELECT … FROM ticket` has ONE cache key, and without the reset the rows
// Ada fetched are the rows Grace renders — one user's data shown to another,
// with no error anywhere. `reset()` kills every live subscription (a LIVE
// SELECT captures its permission context when it is opened, and cannot be
// re-pointed at a new identity), puts every subscribed entry back to pending,
// and re-runs it under the new identity. It is awaited before `viewer` moves,
// so the label and the rows change together.
// ---------------------------------------------------------------------------

import { getQueryClient } from "@surrealguard/query";
import { db } from "./db";

/** The two seeded logins. Both have the password `demo` — see `scripts/seed.surql`. */
export const LOGINS = [
  { name: "Ada", team: "Red", email: "ada@example.com", password: "demo" },
  { name: "Grace", team: "Blue", email: "grace@example.com", password: "demo" },
] as const;

export type Login = (typeof LOGINS)[number];

/** `root` is a SYSTEM user, and system users bypass table permissions entirely. */
export type Viewer = { kind: "root" } | { kind: "person"; name: string; team: string };

let viewer = $state<Viewer>({ kind: "root" });
let busy = $state(false);
let failure = $state<string | undefined>(undefined);

async function becomeViewer(next: Viewer, signIn: () => Promise<unknown>): Promise<void> {
  busy = true;
  failure = undefined;
  try {
    await signIn();
    // Everything the previous identity saw is now wrong. Throw it away and
    // re-run what is still on screen BEFORE announcing the new viewer.
    await getQueryClient(db).reset();
    viewer = next;
  } catch (cause) {
    failure = cause instanceof Error ? cause.message : String(cause);
  } finally {
    busy = false;
  }
}

export const session = {
  get viewer() {
    return viewer;
  },
  get busy() {
    return busy;
  },
  get error() {
    return failure;
  },

  /** Run the ACCESS definition's SIGNIN query. `$auth` becomes that person. */
  signIn(login: Login): Promise<void> {
    return becomeViewer({ kind: "person", name: login.name, team: login.team }, () =>
      db.signin({
        namespace: "demo",
        database: "demo",
        access: "staff",
        variables: { email: login.email, password: login.password },
      }),
    );
  },

  /**
   * Back to root.
   *
   * Note this is NOT `db.invalidate()` — SurrealGuard's `invalidate` means
   * "these queries are stale", while the SDK's `Surreal.invalidate()` drops the
   * session's authentication. Dropping it here would leave the socket
   * anonymous, and this server refuses anonymous queries, so the demo would go
   * blank rather than back to root. Signing in again is the honest move.
   */
  signOut(): Promise<void> {
    return becomeViewer({ kind: "root" }, async () => {
      await db.signin({ username: "root", password: "root" });
      await db.use({ namespace: "demo", database: "demo" });
    });
  },
};
