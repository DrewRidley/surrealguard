# SurrealGuard LSP — TODO

## High Priority
- [ ] Go-to-definition (Cmd+click → jump to DEFINE statement, cross-file)
- [ ] Signature help (show function parameters while typing)
- [ ] Document symbols (outline view of all DEFINE statements)

## Medium Priority
- [ ] Find all references (show all usages of a table/field/variable)
- [ ] Function return validation ("not all code paths return a value")
- [ ] Completion gaps (ORDER BY, GROUP BY, FETCH, SPLIT, OMIT, RETURN clause, WITH INDEX)
- [ ] Code actions / quick fixes (auto-fix typos, add missing DEFINE FIELD)

## Polish
- [ ] Cross-file "defined here" links (track source file URI per definition)
- [ ] Rename symbol (rename table/field everywhere)
- [ ] Workspace-wide diagnostics on save (re-analyze all files when schema changes)
- [ ] Const analysis / dead code detection (IF true, IF 9 = 9)
