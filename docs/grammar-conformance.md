# Grammar conformance report (2026-09-08)

Corpus: 319 known-valid queries extracted from SurrealDB's own test
suites (parser tests, language tests, integration tests, covered.surql);
checked in at crates/syntax/examples/conformance_corpus.json.
Runner: cargo run -p surrealguard-syntax --example conformance -- <corpus.json>
Gate: cargo test -p surrealguard-syntax --test conformance (ratchet against
tests/conformance_expected_failures.txt; regenerate with UPDATE_SNAPSHOTS=1).

**21/319 fail to parse** in the tree-sitter grammar (down from 165 on
2026-07-09). Every remaining failure is a junk extraction, not SurrealQL:

1. **Trailing `\`** line-continuation artifacts (13 entries: #169, #177,
   #180, #186, #189, #192, #198, #203, #242, #243, #291, #292, #293).
2. **Fragments**: bare `}`, `]`, `a:[`, `SELECT * FROM`, `RETURN RETRUN
   FETCH RETURN` (#85, #130, #131, #133, #244).
3. **`PASSHASH .*`** regex placeholders from test fixtures (#283, #285, #287).

Closed since the previous report: DEFINE ACCESS / USER / SEQUENCE, ACCESS
GRANT/SHOW/REVOKE/PURGE, DEFINE NS/DB short forms, COMMENT on every DEFINE,
WITH INDEX/NOINDEX on SELECT/UPDATE/UPSERT/DELETE, DELETE FROM, INSERT INTO
$param / (SELECT …), FULLTEXT ANALYZER, HNSW DISTANCE and DISKANN options,
RATELIMIT, event ASYNC/RETRY/MAXDEPTH and THEN RETURN, KILL $param,
LIVE SELECT … FROM $param, `%`, prefix `NOT`/`-`/`+` on any operand, `?~`,
`1.5dec`, `s'…'`, `?.`, `...`, bare `not()`/`sleep()`, module constants
(`math::pi`, `MaTh::Pi`), `array<T, N>`, IF as a value, `|t:1..10|` sources,
FETCH with a filter, `SET a.b += 1` / `+?=`, INFO FOR USER/INDEX, ALTER INDEX,
SHOW CHANGES FOR DATABASE, comma-separated PERMISSIONS FOR groups.

## Raw failures

```
#85: ERROR at 14..26: `FETCH RETURN`
    in: RETURN RETRUN FETCH RETURN
#130: ERROR at 0..1: `}`
    in: }
#131: ERROR at 0..3: `a:[`
    in: a:[
#133: ERROR at 0..1: `]`
    in: ]
#169: ERROR at 63..64: `\`
    in: DEFINE FUNCTION fn::greet() { RETURN 'Hello' } PERMISSIONS FULL\
#177: ERROR at 144..145: `\`
    in: DEFINE ACCESS access ON NAMESPACE TYPE JWT ALGORITHM HS512 KEY '[REDACTED]' WITH
#180: ERROR at 143..144: `\`
    in: DEFINE ACCESS access ON DATABASE TYPE JWT ALGORITHM HS512 KEY '[REDACTED]' WITH 
#186: ERROR at 100..101: `\`
    in: DEFINE USER user ON NAMESPACE PASSHASH 'secret' ROLES VIEWER DURATION FOR TOKEN 
#189: ERROR at 99..100: `\`
    in: DEFINE USER user ON DATABASE PASSHASH 'secret' ROLES VIEWER DURATION FOR TOKEN 1
#192: ERROR at 48..49: `\`
    in: DEFINE PARAM $param VALUE 'foo' PERMISSIONS FULL\
#198: ERROR at 54..55: `\`
    in: DEFINE EVENT event ON `TB` WHEN true THEN RETURN 'foo'\
#203: ERROR at 29..30: `\`
    in: INFO FOR INDEX field1 ON aaa;\ 			count(SELECT * FROM aaa WHERE field1 @@ 'cupca
#242: ERROR at 28..29: `\`
    in: REMOVE TABLE IF EXISTS node;\n
#243: ERROR at 14..15: `\`
    in: CREATE node:0;\n
#244: ERROR at 13..13: ``
    in: SELECT * FROM
#283: ERROR at 25..36: `PASSHASH .*`
    in: DEFINE USER user ON ROOT PASSHASH .* ROLES VIEWER
#285: ERROR at 30..41: `PASSHASH .*`
    in: DEFINE USER user ON NAMESPACE PASSHASH .* ROLES VIEWER
#287: ERROR at 29..40: `PASSHASH .*`
    in: DEFINE USER user ON DATABASE PASSHASH .* ROLES VIEWER
#291: ERROR at 94..95: `\`
    in: DEFINE USER user ON ROOT PASSHASH 'secret' ROLES VIEWER DURATION FOR TOKEN 15m, 
#292: ERROR at 99..100: `\`
    in: DEFINE USER user ON NAMESPACE PASSHASH 'secret' ROLES VIEWER DURATION FOR TOKEN 
#293: ERROR at 98..99: `\`
    in: DEFINE USER user ON DATABASE PASSHASH 'secret' ROLES VIEWER DURATION FOR TOKEN 1
---
21/319 corpus entries fail to parse
```
