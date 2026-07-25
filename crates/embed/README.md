# surrealguard-embed

Embedded-SurrealQL extraction for
[SurrealGuard](https://github.com/DrewRidley/surrealguard) host adapters.

Host code keeps SurrealQL in `surql` tagged-template literals
(`` surql`SELECT * FROM person` ``). This crate finds those templates with the
host language's own tree-sitter grammar (TypeScript today, including the
`<script>` blocks of Svelte/Vue/Astro), rewrites `${...}` substitutions into
analyzer-visible parameters, and keeps a byte-precise map from the extracted
query back to the host file — so findings computed on the query render at the
right host spans.

The public API is not yet stable; pin an exact version.

## License

MIT OR Apache-2.0.
