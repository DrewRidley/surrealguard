# surrealguard-embed

Embedded-SurrealQL extraction for
[SurrealGuard](https://github.com/DrewRidley/surrealguard) host adapters.

Host code keeps SurrealQL in a string literal passed to a query sink —
`db.query("SELECT * FROM person")`, `defineQuery("...")`, `defineLive("...")`.
This crate finds those calls with the host language's own tree-sitter grammar
(TypeScript today, including the `<script>` blocks of Svelte/Vue/Astro),
rewrites the `${...}` substitutions of a template argument into
analyzer-visible parameters, and keeps a byte-precise map from the extracted
query back to the host file — so findings computed on the query render at the
right host spans.

A string literal and not a tagged template: `TemplateStringsArray` has no
generic parameter, so `` surql`…` `` loses its query text before TypeScript's
inference can read it, and the generated registry has nothing to key on.

The public API is not yet stable; pin an exact version.

## License

MIT OR Apache-2.0.
