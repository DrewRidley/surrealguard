// "Is the database actually there?" — asked over HTTP, because the WebSocket
// cannot answer it.
//
// `Surreal.connect()` does not reject when nothing is listening; it waits. A
// page opened before `pnpm db` therefore sits on four spinners forever, with no
// error for any `error` snippet to render. This polls SurrealDB's `/health`
// endpoint so the app can say the one useful thing instead.
//
// It is demo scaffolding, not something an app needs. A deployed app has a
// database that is already running.

import { HEALTH_URL } from "./db";

let reachable = $state<boolean | undefined>(undefined);
/** Whether the FIRST answer was "no". That is the case a reload has to fix. */
let startedDown = $state(false);

export const health = {
  /** `undefined` until the first poll answers. */
  get reachable() {
    return reachable;
  },

  /**
   * The database is up, but this page was loaded while it was down, so the
   * client is still waiting on a socket it opened against nothing. A dropped
   * connection reconnects on its own; a connection that never opened does not.
   */
  get needsReload() {
    return startedDown && reachable === true;
  },

  /** Poll until stopped. Call from an `$effect` so it tears down with the page. */
  watch(intervalMs = 1500): () => void {
    let stopped = false;
    let first = true;
    const tick = async () => {
      let ok = false;
      try {
        ok = (await fetch(HEALTH_URL, { cache: "no-store" })).ok;
      } catch {
        ok = false;
      }
      if (stopped) return;
      if (first) {
        first = false;
        startedDown = !ok;
      }
      reachable = ok;
    };
    void tick();
    const timer = setInterval(() => void tick(), intervalMs);
    return () => {
      stopped = true;
      clearInterval(timer);
    };
  },
};
