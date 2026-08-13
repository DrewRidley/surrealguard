#!/usr/bin/env node
/**
 * One command, one database: start SurrealDB, apply `schema/schema.surql`,
 * apply `scripts/seed.surql`, then stay in the foreground so Ctrl-C stops it.
 *
 * Two decisions worth naming:
 *
 * 1. **Port 8124, not 8000.** A demo machine usually already has something on
 *    the default port, and discovering that from a blank page with an audience
 *    watching is not a debugging session worth having. `src/lib/db.ts` points
 *    at the same port; they are the only two places it appears.
 * 2. **In-memory.** Restarting the process is the reset button, and there is no
 *    stale file to explain when yesterday's rows show up today.
 *
 * `--seed-only` skips starting a server and re-seeds one that is already
 * running — the "put it back how it was" command between rehearsals.
 */

import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { Surreal } from "surrealdb";

const HERE = dirname(fileURLToPath(import.meta.url));
const ROOT = join(HERE, "..");

export const PORT = 8124;
export const URL_RPC = `ws://127.0.0.1:${PORT}/rpc`;
export const URL_HEALTH = `http://127.0.0.1:${PORT}/health`;
export const NAMESPACE = "demo";
export const DATABASE = "demo";
export const ROOT_USER = "root";
export const ROOT_PASS = "root";

const seedOnly = process.argv.includes("--seed-only");

/** Poll `/health` until the server answers, or give up with a usable message. */
async function waitForHealth(timeoutMs = 20_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      const response = await fetch(URL_HEALTH);
      if (response.ok) return;
    } catch {
      // not up yet
    }
    if (Date.now() > deadline) {
      throw new Error(`SurrealDB did not become healthy on ${URL_HEALTH} within ${timeoutMs}ms`);
    }
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
}

/** Apply the schema and the seed, in that order, as root. */
export async function seed() {
  const db = new Surreal();
  await db.connect(URL_RPC);
  await db.signin({ username: ROOT_USER, password: ROOT_PASS });
  await db.use({ namespace: NAMESPACE, database: DATABASE });

  // Drop the whole database first, so `--seed-only` is idempotent: a bare
  // `DEFINE ACCESS` errors if one already exists, and re-running the schema on
  // a live database is exactly the case that would hit.
  await db.query(`REMOVE DATABASE IF EXISTS ${DATABASE};`);
  await db.use({ namespace: NAMESPACE, database: DATABASE });

  const schema = await readFile(join(ROOT, "schema", "schema.surql"), "utf8");
  const data = await readFile(join(ROOT, "scripts", "seed.surql"), "utf8");
  await db.query(schema);
  await db.query(data);

  const [people] = await db.query("SELECT id FROM person");
  const [tickets] = await db.query("SELECT id FROM ticket");
  await db.close();
  return { people: people.length, tickets: tickets.length };
}

async function main() {
  let server;
  if (!seedOnly) {
    server = spawn(
      "surreal",
      ["start", "--bind", `127.0.0.1:${PORT}`, "--user", ROOT_USER, "--pass", ROOT_PASS, "memory"],
      { stdio: ["ignore", "inherit", "inherit"] },
    );
    server.on("error", (cause) => {
      console.error(
        `\n  Could not run \`surreal\`. Install it first:\n` +
          `    curl -sSf https://install.surrealdb.com | sh\n\n  ${cause.message}\n`,
      );
      process.exit(1);
    });
    const stop = () => {
      server?.kill("SIGTERM");
      process.exit(0);
    };
    process.on("SIGINT", stop);
    process.on("SIGTERM", stop);
  }

  await waitForHealth();
  const counts = await seed();

  console.log(
    `\n  SurrealDB ready on ${URL_RPC}` +
      `\n  namespace=${NAMESPACE} database=${DATABASE} (root/root)` +
      `\n  seeded ${counts.people} people, ${counts.tickets} tickets` +
      (seedOnly ? "\n" : "\n\n  Leave this running. Ctrl-C stops it.\n"),
  );

  if (seedOnly) process.exit(0);
  // Keep the process alive for as long as the server lives.
  await new Promise((resolve) => server.on("exit", resolve));
}

// Only run when invoked directly, so `seed()` is importable by the checks in
// `scripts/verify.mjs`.
if (process.argv[1] && fileURLToPath(import.meta.url) === process.argv[1]) {
  main().catch((cause) => {
    console.error(cause);
    process.exit(1);
  });
}
