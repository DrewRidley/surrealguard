#!/usr/bin/env node
/**
 * Drive the demo headlessly, against a real SurrealDB, through the same code
 * the page runs.
 *
 * "The same code" is meant literally in two places:
 *
 * - the queries are the ones the PREPROCESSOR emits for
 *   `src/routes/+page.svelte`. This runs `@surrealdb/analyzer-svelte/preprocess` over
 *   that file, pulls the `__sg_query(...)` / `__sg_live(...)` calls out of the
 *   result, and runs those. If the attribute changes, this follows it; there is
 *   no second copy of the query text anywhere.
 * - `<Query>` is `createQuery` is `getQueryClient(db).observe(...)`, and
 *   `<LiveQuery>` is `createLive` is `.observeLive(...)`, which is what is
 *   subscribed below.
 *
 *   node scripts/verify.mjs      # needs `pnpm db` running in another terminal
 */

import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { createClient, defineQuery, RecordId } from "@surrealdb/analyzer-client";
import { sgText, sgTextLive } from "@surrealdb/analyzer-svelte/inline";
import { getQueryClient } from "@surrealdb/analyzer-query";
import { DATABASE, NAMESPACE, ROOT_PASS, ROOT_USER, seed, URL_RPC } from "./db.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

let failures = 0;
function check(label, ok, detail = "") {
  console.log(`  ${ok ? "ok  " : "FAIL"}  ${label}${detail ? `  — ${detail}` : ""}`);
  if (!ok) failures += 1;
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

async function until(predicate, timeoutMs = 5000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (predicate()) return true;
    await sleep(50);
  }
  return false;
}

/**
 * The queries the demo page actually shows, read off its `q="…"` attributes.
 *
 * Read from the source rather than restated here, so this cannot drift into
 * testing a query the page does not run — which is the only reason it is worth
 * running at all.
 */
async function inlineQueriesOfDemoPage() {
  const content = await readFile(join(ROOT, "src", "routes", "+page.svelte"), "utf8");
  const found = [];
  const attribute = /<(Query|LiveQuery)\s[^>]*?q="([^"]+)"/g;
  for (const [, element, text] of content.matchAll(attribute)) {
    found.push({ kind: element === "LiveQuery" ? "live" : "query", text });
  }
  return found;
}

async function main() {
  console.log("\nre-seeding, so the numbers below mean something…");
  await seed();

  // ---- the preprocessor, on the file the demo actually shows --------------
  console.log("\nthe inline attribute");
  const inline = await inlineQueriesOfDemoPage();
  // By what they select, not by where they sit: the page has three inline
  // queries and adding a fourth must not silently renumber these.
  const from = (table) => inline.find((entry) => entry.text.includes(`FROM ${table}`));
  const roster = from("person");
  const tickets = from("ticket");
  const teams = from("team");
  check("+page.svelte subscribes to the roster", roster?.kind === "live");
  check("+page.svelte queries the tickets", tickets?.kind === "query");
  check("+page.svelte queries the team picker", teams?.kind === "query");
  if (!roster || !tickets || !teams) {
    throw new Error("the demo page no longer has all three inline queries");
  }

  const rosterAt = (minAge) => sgTextLive(roster.text, { min: minAge });
  check(
    "the value is bound, not spliced into the text",
    rosterAt(30).text.includes("$min") && !rosterAt(30).text.includes("30"),
    rosterAt(30).text,
  );
  check("every slider position is ONE query text", rosterAt(30).text === rosterAt(31).text);
  check("…and two different cache keys", rosterAt(30).key !== rosterAt(31).key);
  check("the parameter is bound under the name the query spells", "min" in (rosterAt(30).params ?? {}));

  const db = createClient({
    url: URL_RPC,
    namespace: NAMESPACE,
    database: DATABASE,
    authentication: { username: ROOT_USER, password: ROOT_PASS },
  });
  const core = getQueryClient(db);

  // ---- the add form's team picker is a query, not a list ------------------
  // The `<select>` is filled from this. Come back empty and the form has no
  // team to offer, so "add a person" cannot be driven at all.
  console.log("\nthe team picker");
  const teamRows = await core.fetch(sgText(teams.text, undefined));
  check(
    "the picker's options are rows of `team`",
    teamRows.length === 2 && teamRows.every((row) => typeof row.name === "string"),
    teamRows.map((row) => row.name).join(", "),
  );

  // ---- a parameter change re-runs the query ------------------------------
  console.log("\nthe slider");
  const namesAbove = async (minAge) => {
    const observable = core.observeLive(rosterAt(minAge));
    const stop = observable.subscribe(() => {});
    await until(() => observable.get().status === "success");
    const names = observable.get().data.map((row) => row.name).sort();
    stop();
    return names;
  };
  const above20 = await namesAbove(20);
  const above40 = await namesAbove(40);
  const above46 = await namesAbove(46);
  check("age > 20 → all five", above20.length === 5, String(above20));
  check("age > 40 → Alan and Grace", String(above40) === "Alan,Grace", String(above40));
  check("age > 46 → nobody", above46.length === 0, String(above46));

  // ---- a value with a quote in it ----------------------------------------
  // The proof that this is a bound parameter rather than string concatenation:
  // spliced into the text, `O'Hara "the Bold"` is a syntax error, and a
  // determined value would be an injection.
  console.log("\na hostile value");
  const HOSTILE = `O'Hara "the Bold"`;
  const addPerson = defineQuery.unchecked(
    "CREATE person SET name = $name, age = $age, team = $team",
  );
  const [hostilePerson] = await core.mutate(addPerson, {
    name: HOSTILE,
    age: 44,
    team: new RecordId("team", "red"),
  });
  const byName = sgText("SELECT id, name FROM person WHERE name = $name", { name: HOSTILE });
  check("the quote never enters the query text", !byName.text.includes("O'Hara"), byName.text);
  const matched = await core.fetch(byName);
  check("and the row still comes back", matched.length === 1 && matched[0].name === HOSTILE);
  await core.mutate(defineQuery.unchecked("DELETE person WHERE id = $person"), {
    person: hostilePerson.id,
  });

  // ---- a write reaches the live subscription ------------------------------
  console.log("\nlive updates");
  const live = core.observeLive(rosterAt(20));
  const stopLive = live.subscribe(() => {});
  await until(() => live.get().status === "success");
  // `status: success` means the SEEDING SELECT resolved, which happens before
  // the `LIVE SELECT` is open — so a write issued the instant the rows appear
  // can land in the gap and never be notified. The page cannot hit this (a
  // human takes longer than a round trip to reach the button) but a script
  // does, every time.
  await sleep(500);
  const before = live.get().data.length;
  const [created] = await core.mutate(addPerson, {
    name: "Edsger",
    age: 42,
    team: new RecordId("team", "red"),
  });
  check(
    `CREATE arrived over the subscription (${before} → ${before + 1})`,
    await until(() => live.get().data.length === before + 1),
  );
  check(
    "the new row is the one that was written",
    live.get().data.some((row) => row.name === "Edsger"),
  );
  await core.mutate(defineQuery.unchecked("DELETE person WHERE id = $person"), {
    person: created.id,
  });
  check(
    `DELETE arrived over the subscription (${before + 1} → ${before})`,
    await until(
      () =>
        live.get().data.length === before &&
        !live.get().data.some((row) => row.name === "Edsger"),
    ),
  );

  // The filter is the database's, not the page's: someone below the threshold
  // must not appear at all.
  const filtered = core.observeLive(rosterAt(40));
  const stopFiltered = filtered.subscribe(() => {});
  await until(() => filtered.get().status === "success");
  await sleep(500);
  const filteredBefore = filtered.get().data.length;
  const [young] = await core.mutate(addPerson, {
    name: "Margaret",
    age: 33,
    team: new RecordId("team", "red"),
  });
  await sleep(700);
  check(
    "a row below the threshold never reaches the filtered subscription",
    filtered.get().data.length === filteredBefore,
    `${filtered.get().data.length}`,
  );
  await core.mutate(defineQuery.unchecked("DELETE person WHERE id = $person"), {
    person: young.id,
  });
  stopFiltered();

  // ---- identity changes what the same query returns -----------------------
  console.log("\nrecord-level access control");
  const ticketQuery = () => sgText(tickets.text, undefined);
  const asRoot = core.observe(ticketQuery());
  const stopTickets = asRoot.subscribe(() => {});
  await until(() => asRoot.get().status === "success");
  check("root sees all 6 tickets", asRoot.get().data.length === 6, `${asRoot.get().data.length}`);

  const signIn = (email) =>
    db.signin({
      namespace: NAMESPACE,
      database: DATABASE,
      access: "staff",
      variables: { email, password: "demo" },
    });

  await signIn("ada@example.com");
  // The hazard, before it is closed: a cache key is the query text plus its
  // parameters, and `$auth` is in neither.
  check(
    "without a reset, Ada would still be rendering root's 6 rows",
    asRoot.get().data.length === 6,
    "this is the leak `reset()` closes",
  );
  await core.reset();
  await until(() => asRoot.get().status === "success");
  const ada = asRoot.get().data;
  check("after reset(), Ada sees 3", ada.length === 3, `${ada.length}`);
  check("all of them team:red", ada.every((row) => row.team === "team:red"));

  // A LIVE SELECT captures its permission context when it opens, so `reset()`
  // has to re-establish it rather than leave it running under the old identity.
  const liveCount = live.get().data.length;
  const [asAda] = await core.mutate(addPerson, {
    name: "Radia",
    age: 38,
    team: new RecordId("team", "blue"),
  });
  check(
    "the live subscription still delivers after an identity change",
    await until(() => live.get().data.length === liveCount + 1),
  );
  await core.mutate(defineQuery.unchecked("DELETE person WHERE id = $person"), {
    person: asAda.id,
  });
  await until(() => live.get().data.length === liveCount);

  await signIn("grace@example.com");
  await core.reset();
  const grace = asRoot.get().data;
  check("Grace sees 3", grace.length === 3, `${grace.length}`);
  check("all of them team:blue", grace.every((row) => row.team === "team:blue"));
  check(
    "none of Ada's rows survived into Grace's view",
    grace.every((row) => !ada.some((other) => other.id === row.id)),
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

  console.log(failures === 0 ? "\nall checks passed\n" : `\n${failures} CHECK(S) FAILED\n`);
  process.exit(failures === 0 ? 0 : 1);
}

main().catch((cause) => {
  console.error(
    `\n${cause}\n\n  Is the database running? Start it with \`pnpm db\` in another terminal.\n`,
  );
  process.exit(1);
});
