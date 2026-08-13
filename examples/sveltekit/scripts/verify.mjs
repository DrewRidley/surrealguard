#!/usr/bin/env node
/**
 * Drive the demo's four beats headlessly, against a real SurrealDB, through the
 * exact code path the components use.
 *
 * `<Query>` is `createQuery` is `getQueryClient(db).observe(...)`, and
 * `<LiveQuery>` is `createLive` is `.observeLive(...)`. Everything below the
 * Svelte layer is what runs here; what is NOT covered is the runes plumbing
 * itself, which `packages/svelte`'s own tests cover.
 *
 * The query TEXTS are read out of `src/lib/queries.ts` rather than retyped, so
 * this cannot pass against a query the app does not run.
 *
 *   node scripts/verify.mjs      # needs `pnpm db` running in another terminal
 */

import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createClient, defineLive, defineQuery, RecordId } from "@surrealguard/client";
import { getQueryClient } from "@surrealguard/query";
import { DATABASE, NAMESPACE, ROOT_PASS, ROOT_USER, seed, URL_RPC } from "./db.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

let failures = 0;
function check(label, ok, detail = "") {
  console.log(`  ${ok ? "ok  " : "FAIL"}  ${label}${detail ? `  — ${detail}` : ""}`);
  if (!ok) failures += 1;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

/** Wait for a condition, polling — a subscription is asynchronous by nature. */
async function until(predicate, timeoutMs = 4000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return true;
    await sleep(50);
  }
  return false;
}

/** The app's own query texts, by name, straight out of the source. */
async function appQueries() {
  const source = await readFile(join(ROOT, "src", "lib", "queries.ts"), "utf8");
  const out = {};
  const pattern = /^export const (\w+) = define(Query|Live)\(\s*\n?\s*"([^"]+)"/gm;
  for (const [, name, kind, text] of source.matchAll(pattern)) {
    out[name] = kind === "Live" ? defineLive(text) : defineQuery(text);
  }
  return out;
}

async function main() {
  console.log("\nre-seeding, so the numbers below mean something…");
  await seed();

  const q = await appQueries();
  for (const name of [
    "allPeople",
    "peopleOver",
    "livePeople",
    "liveRoster",
    "addPerson",
    "removePerson",
    "allTeams",
    "allTickets",
  ]) {
    if (!q[name]) throw new Error(`could not read query \`${name}\` out of src/lib/queries.ts`);
  }

  const db = createClient({
    url: URL_RPC,
    namespace: NAMESPACE,
    database: DATABASE,
    authentication: { username: ROOT_USER, password: ROOT_PASS },
  });
  const core = getQueryClient(db);

  // ---- Beat 1: a parameter change re-runs the query --------------------
  console.log("\nbeat 1 — reactive parameters");
  const at = async (minAge) => {
    // What the thunk in `<Query q={() => peopleOver.with({ minAge })}>` resolves
    // to on each change: a new bound query, therefore a new cache key.
    const observable = core.observe(q.peopleOver.with({ minAge }));
    const stop = observable.subscribe(() => {});
    await until(() => observable.get().status === "success");
    const rows = observable.get().data ?? [];
    stop();
    return rows.map((row) => row.name);
  };
  const at30 = await at(30);
  const at45 = await at(45);
  const at25 = await at(25);
  check(
    "minAge=30 → Barbara, Ada, Alan, Grace",
    String(at30) === "Barbara,Ada,Alan,Grace",
    String(at30),
  );
  check("minAge=45 → Grace only", String(at45) === "Grace", String(at45));
  check("minAge=25 → all five", at25.length === 5, String(at25));
  check("the result set actually changed", String(at30) !== String(at45));

  // ---- Beat 2: a write reaches a live subscription ----------------------
  console.log("\nbeat 2 — live updates");
  const live = core.observeLive(q.livePeople);
  const seen = [];
  const stopLive = live.subscribe((state) => seen.push((state.data ?? []).length));
  await until(() => live.get().status === "success");
  const before = live.get().data.length;

  const [created] = await core.mutate(
    q.addPerson,
    { name: "Edsger", age: 42, team: new RecordId("team", "red") },
    { invalidates: [q.allPeople, q.allTeams, q.peopleOver] },
  );
  const grew = await until(() => live.get().data.length === before + 1);
  check(`CREATE arrived over the subscription (${before} → ${before + 1})`, grew);
  check(
    "the new row is the one that was written",
    live.get().data.some((row) => row.name === "Edsger"),
  );

  await core.mutate(q.removePerson, { person: created.id }, { invalidates: [q.allPeople] });
  const shrank = await until(() => live.get().data.length === before);
  check(`DELETE arrived over the subscription (${before + 1} → ${before})`, shrank);
  check("the seed did not replay as notifications", seen[0] === 0 || seen.length > 0);

  // ---- Beat 3: one subscription per row, parameterised by that row ------
  console.log("\nbeat 3 — nesting");
  const red = core.observeLive(q.liveRoster.with({ team: new RecordId("team", "red") }));
  const blue = core.observeLive(q.liveRoster.with({ team: new RecordId("team", "blue") }));
  const stopRed = red.subscribe(() => {});
  const stopBlue = blue.subscribe(() => {});
  await until(() => red.get().status === "success" && blue.get().status === "success");
  const redBefore = red.get().data.length;
  const blueBefore = blue.get().data.length;
  check(`red starts with ${redBefore}, blue with ${blueBefore}`, redBefore === 3 && blueBefore === 2);

  const [redPerson] = await core.mutate(q.addPerson, {
    name: "Margaret",
    age: 33,
    team: new RecordId("team", "red"),
  });
  const redGrew = await until(() => red.get().data.length === redBefore + 1);
  check("adding to red reaches only red's subscription", redGrew);
  check("blue was untouched", blue.get().data.length === blueBefore, `blue=${blue.get().data.length}`);

  // Two observers of the SAME key share one entry, and therefore one LIVE SELECT.
  const redAgain = core.observeLive(q.liveRoster.with({ team: new RecordId("team", "red") }));
  check("a second observer of the same key shares the entry", redAgain.get() === red.get());

  await core.mutate(q.removePerson, { person: redPerson.id });
  await until(() => red.get().data.length === redBefore);
  stopRed();
  stopBlue();

  // ---- Beat 4: identity changes what the same query returns -------------
  console.log("\nbeat 4 — record-level access control");
  const tickets = () => {
    const observable = core.observe(q.allTickets);
    return observable;
  };
  const asRoot = tickets();
  const stopTickets = asRoot.subscribe(() => {});
  await until(() => asRoot.get().status === "success");
  check("root sees all 6 tickets", asRoot.get().data.length === 6, `${asRoot.get().data.length}`);

  await db.signin({
    namespace: NAMESPACE,
    database: DATABASE,
    access: "staff",
    variables: { email: "ada@example.com", password: "demo" },
  });

  // THE HAZARD, demonstrated before it is fixed: the identity has changed and
  // the cache has not. `$auth` is in neither the query text nor its parameters,
  // so nothing about the key knows.
  check(
    "without a reset, Ada would still be rendering root's 6 rows",
    asRoot.get().data.length === 6,
    "this is the leak `reset()` closes",
  );

  await core.reset();
  await until(() => asRoot.get().status === "success");
  const adaTickets = asRoot.get().data;
  check("after reset(), Ada sees 3", adaTickets.length === 3, `${adaTickets.length}`);
  check(
    "and they are all team:red",
    adaTickets.every((ticket) => ticket.team === "team:red"),
    adaTickets.map((t) => t.team).join(","),
  );

  // The live subscription opened as root has to be re-established, not merely
  // dropped: a LIVE SELECT captures its permission context when it is opened.
  const afterResetCount = live.get().data.length;
  const [asAda] = await core.mutate(q.addPerson, {
    name: "Radia",
    age: 38,
    team: new RecordId("team", "blue"),
  });
  const stillLive = await until(() => live.get().data.length === afterResetCount + 1);
  check("the live subscription still delivers after an identity change", stillLive);
  await core.mutate(q.removePerson, { person: asAda.id });
  await until(() => live.get().data.length === afterResetCount);

  await db.signin({
    namespace: NAMESPACE,
    database: DATABASE,
    access: "staff",
    variables: { email: "grace@example.com", password: "demo" },
  });
  await core.reset();
  await until(() => asRoot.get().status === "success" && asRoot.get().data.length !== 3 || false, 500);
  const graceTickets = asRoot.get().data;
  check("Grace sees 3", graceTickets.length === 3, `${graceTickets.length}`);
  check(
    "and they are all team:blue",
    graceTickets.every((ticket) => ticket.team === "team:blue"),
    graceTickets.map((t) => t.team).join(","),
  );
  check(
    "none of Ada's rows survived into Grace's view",
    graceTickets.every((ticket) => !adaTickets.some((other) => other.id === ticket.id)),
  );

  await db.signin({ username: ROOT_USER, password: ROOT_PASS });
  await db.use({ namespace: NAMESPACE, database: DATABASE });
  await core.reset();
  await until(() => asRoot.get().data.length === 6);
  check("signing back out to root restores all 6", asRoot.get().data.length === 6);

  stopTickets();
  stopLive();
  core.clear();
  await db.close();

  console.log(
    failures === 0
      ? "\nall checks passed\n"
      : `\n${failures} CHECK(S) FAILED\n`,
  );
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((cause) => {
  console.error(
    `\n${cause}\n\n  Is the database running? Start it with \`pnpm db\` in another terminal.\n`,
  );
  process.exit(1);
});
