# LSP hover / presentation polish — pre-publish backlog (2026-07-24)

Captured from live Zed usage. **Do these AFTER the in-flight hover
reformatting lands** (owned in `crates/lsp/src/backend.rs` +
`crates/workspace/src/query.rs`) — do not start while that change is open.
Target: land before the publish.

## Observed (record hover, e.g. `$parent: record<organization>`)

The hover lists each field on its own line with a code-block background, e.g.:

```
fields
  billing: any
  country: option<record<country>>
  legal_name: string
  sectors: array<record<sector>>
  ...
```

## Punch list

1. **No syntax highlighting on the RHS type.** Each field renders with a
   code-block background, but the kind on the right of `:` is not tokenized —
   it reads as flat text. Investigate emitting the hover body inside a fenced
   block with a language the client highlights (e.g. ```` ```surql ````), or
   otherwise structuring the `MarkupContent` so kinds (`record<country>`,
   `option<...>`, `array<...>`) get colored. This is the highest-visibility
   item.

2. **Section header casing.** The `fields` label should be capitalized —
   `Fields` (and audit any other lowercase section headers in the hover for
   consistency).

3. **Show field PERMISSIONS when simple.** The hover omits permissions today.
   Surface them when they are cheap to render — e.g. a compact
   `permissions: full` / `none`, or the `FOR select/create/update/delete`
   predicate when short. Collapse or omit when the predicate is complex so the
   hover stays readable. Nice-to-have, gated on it not bloating the block.

4. **Keep iterating on LSP quality generally.** Hover is the most-seen
   surface; treat this list as a starting point, not the whole scope, for a
   pre-publish LSP polish pass.

## Notes

- The inline **diagnostic** message is plain text per LSP spec (Zed renders it
  literally), so rich/markdown formatting for kinds belongs in the **hover**
  (`MarkupContent`, markdown), not in `crates/lsp/src/diagnostics.rs`.
- Reminder: none of this shows in Zed until `surrealql-analyzer-lsp` is rebuilt and
  the language server is restarted.
