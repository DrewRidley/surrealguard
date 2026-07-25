# surrealguard-diagnostics

Findings, severities, codes, and lint policy for
[SurrealGuard](https://github.com/DrewRidley/surrealguard).

A `Finding` carries only its intrinsic severity class; consumers (CLI, LSP, host
adapters) resolve the *effective* severity through a `PolicyConfig` at their
edge. Codes are allocated in a central catalog — one code per contract — and
render with a severity-derived prefix (`E1002`, `W7002`, `S0001`). This crate
also parses inline suppression directives.

The public API is not yet stable; pin an exact version.

## License

MIT OR Apache-2.0.
