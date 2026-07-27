/**
 * `@surrealguard/ts-plugin` — SurrealGuard's diagnostics and highlighting,
 * given as TypeScript's own answers.
 *
 * tsserver `require`s this module and calls the default export with its own
 * `typescript` instance. That instance is the one that must be used for
 * everything: a plugin that imports `typescript` itself gets a *second* copy,
 * whose enums and `SourceFile` shapes are not the ones the host is comparing
 * against.
 *
 * Usage is one entry in a `tsconfig.json`:
 *
 * ```json
 * { "compilerOptions": { "plugins": [{ "name": "@surrealguard/ts-plugin" }] } }
 * ```
 *
 * Plugins load in tsserver — VS Code, Cursor, WebStorm, Zed, `svelte-check` —
 * and **not** in `tsc`. That is by design in TypeScript and it is the right
 * split here: CI should keep running `surrealguard check`, which sees the whole
 * workspace rather than one file at a time.
 */

import type tsModule from "typescript/lib/tsserverlibrary";

import { createProxy } from "./plugin";

function init(modules: { typescript: typeof tsModule }) {
  return {
    create(info: tsModule.server.PluginCreateInfo): tsModule.LanguageService {
      return createProxy(modules.typescript, info);
    },
  };
}

export = init;
