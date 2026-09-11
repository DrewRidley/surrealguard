
import { createClient } from "$lib/surrealql-analyzer.generated";

export const RPC_URL = "ws://127.0.0.1:8124/rpc";


export const HEALTH_URL = "http://127.0.0.1:8124/health";

export const db = createClient({
  url: RPC_URL,
  namespace: "demo",
  database: "demo",
  // `authentication`, not `signin`: the SDK reuses these credentials whenever a
  // session expires or the socket reconnects, so the demo survives a laptop
  // sleeping. A `.signin()` call — which the access-control beat makes — takes
  // precedence over this for the rest of that session.
  //
  // Root, because a root user BYPASSES table permissions: that is what makes
  // "signed out" mean "sees every ticket" in the last beat. A real app would
  // never ship this.
  authentication: { username: "root", password: "root" },
});
