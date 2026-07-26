# Oracle baseline triage — 2026-07-26

Adjudication of all 42 findings in `tests/oracle_baseline.txt` against the
real-world corpus at `/Users/drewridley/Documents/Projects/workshop/database`
(read-only; never modified).

**Verdict counts: 40 `genuine`, 2 `BUG`, 0 `expected`.**

Evidence standard: wherever the claim was testable, it was checked against a
live **SurrealDB 3.0.5** server (`/usr/local/bin/surreal`, started on
`127.0.0.1:18234`, shut down afterwards). Engine transcripts are quoted inline.

The 42 findings collapse into **7 root-cause groups**. Two of those groups
(33 findings) are a single stale refactor each.

---

## Summary table

| # | Group | Findings | Codes | Verdict |
|---|-------|---------:|-------|---------|
| G1 | `loyalty_*` → `commerce_loyalty_*` rename never propagated | 9 | E1001 | genuine |
| G2 | `task_status` refactor never propagated to `project`'s setup event | 15 | E1002 ×10, E2034 ×5 | genuine |
| G3 | `kitchen_station` → `pos_kitchen_station` rename never propagated | 2 | E1001 | genuine |
| G4 | Tables referenced but never written (`product_kind`, `user`, `mailbox_folder`, `passkey`, `task_comment`, `has_billing_module`, `organization_billing_limit`) | 7 | E1001 | genuine |
| G5 | Guards written against fields/kinds that make them dead code | 6 | E1002 ×3, E2004 ×2, W7005 ×1 | genuine |
| G6 | `option<string>` passed to a `string` parameter | 1 | E5002 | genuine |
| G7 | `count()` flagged where the consumer is an existence test | 2 | W4023 | **BUG** |

There is exactly **one BUG** to fix (BUG-1, 2 findings). A second, unrelated
analyzer defect was discovered during triage and is written up as BUG-2 — it
produces *false negatives*, not false positives, so it is not in the baseline.

---

## G1 — the `loyalty_*` rename (9 × E1001, genuine)

The three loyalty tables were renamed to a `commerce_` prefix; the `record<>`
targets that point at them were not updated.

| Defined as | Still referenced as |
|---|---|
| `commerce_loyalty_program` (`loyalty/loyalty_program.surql:1`) | `loyalty_program` |
| `commerce_loyalty_tier` (`loyalty/loyalty_tier.surql:1`) | `loyalty_tier` |
| `commerce_loyalty_reward` (`loyalty/loyalty_reward.surql:1`) | `loyalty_reward` |

Nine dangling sites:

```
apps/commerce/discount.surql:94                     array<record<loyalty_tier>>
apps/commerce/loyalty/loyalty_reward.surql:7        record<loyalty_program>
apps/commerce/loyalty/loyalty_reward.surql:37       option<record<loyalty_tier>>
apps/commerce/loyalty/loyalty_tier.surql:7          record<loyalty_program>
apps/commerce/loyalty/loyalty_transaction.surql:11  record<loyalty_program>
apps/commerce/loyalty/loyalty_transaction.surql:33  option<record<loyalty_reward>>
apps/commerce/loyalty/loyalty_transaction.surql:37  option<record<loyalty_tier>>
apps/crm/customer.surql:70                          option<record<loyalty_program>>
apps/crm/customer.surql:73                          option<record<loyalty_tier>>
```

**Verification.** `grep -rhoE 'DEFINE TABLE( OVERWRITE)?( IF NOT EXISTS)? +\w+'`
over `schema/**` yields 119 table names; `loyalty_program`, `loyalty_tier` and
`loyalty_reward` are not among them, and nothing outside the `schema/**` glob
(`setup.surql`, `seed/`, `tests/`) defines them either. Note that the two
sibling files `loyalty_tier.surql` and `loyalty_reward.surql` *define*
`commerce_loyalty_*` while *referencing* `loyalty_*` in the same file — an
unambiguous half-finished rename.

**Engine note (recorded for honesty).** SurrealDB accepts a dangling record
target; `DEFINE FIELD ghost ON emp TYPE record<no_such_table>` returns `NONE`,
no error. E1001 on a `record<T>` target is therefore a *dangling-reference*
finding, not an engine-rejection finding. It is still `genuine`: the corpus
means `commerce_loyalty_tier`, and every one of these fields can only ever hold
ids into a table that does not exist.

---

## G2 — `project`'s `setup_statuses` event (10 × E1002 + 5 × E2034, genuine)

`schema/suite/project/project.surql:58-65`:

```surql
DEFINE EVENT OVERWRITE setup_statuses ON project WHEN $event = 'CREATE' THEN {
	CREATE task_status SET name = 'Backlog', category = 'backlog', project = $after.id, color = '#f2994a';
	... ×5 ...
};
```

`task_status` (`schema/suite/task/task_status.surql`) is `SCHEMAFULL` with
exactly `template`, `name`, `description`, `color`, `state`. There is no
`category` and no `project` — per-project status now lives in the separate
`project_task_status` join table (`project`, `status`, `position`,
`name_override`, `color_override`). The event is stale.

**Engine-verified — E2034 (`template` must be set).** Replaying the schema and
the first CREATE on 3.0.5:

```
Couldn't coerce value for field `template` of `task_status:0wq5…`:
Expected `record<task_status_template>` but found `NONE`
```

`template` is `TYPE record<task_status_template>` with no `DEFAULT`, so all five
CREATEs abort. **Every project CREATE in this database throws.**

**Engine-verified — E1002 (`category` / `project`).** With `template` supplied so
the coercion passes:

```
Found field 'category', but no such field exists for table 'task_status'
```

SurrealDB 3.0 rejects unknown fields on a SCHEMAFULL table outright (it does not
silently drop them). Both codes describe real engine failures.

---

## G3 — the `kitchen_station` rename (2 × E1001, genuine)

`pos/kitchen/pos_kitchen_station.surql:1` defines `pos_kitchen_station`. Two
fields still name the old table:

```
apps/commerce/order/has_line_item.surql:80             option<record<kitchen_station>>
apps/commerce/pos/kitchen/pos_kitchen_ticket.surql:13  record<kitchen_station>
```

The second is in the same directory as the definition it fails to match.

---

## G4 — tables that were never written (7 × E1001, genuine)

Two sub-shapes, both dangling, but with very different engine consequences.

**G4a — `record<>` / `COMPUTED <~` back-references (5).** Engine-tolerated;
statically dead.

```
apps/commerce/products/service.surql:10           option<record<product_kind>>
organization/organization_delegation_grant.surql:7  record<team | user | organization_unit>
communication/email_address.surql:7               COMPUTED <~mailbox_folder
person/account.surql:68                           COMPUTED <~passkey
suite/task/task.surql:58                          COMPUTED <~task_comment
```

`product_kind`, `user`, `mailbox_folder`, `passkey` and `task_comment` appear in
zero `DEFINE TABLE` statements. `user` is almost certainly meant to be `account`
(the corpus's account table). Verified on 3.0.5 that
`DEFINE FIELD back ON m COMPUTED <~ghost_table` is accepted and evaluates to `[]`
— so these are permanently-empty fields rather than errors.

**G4b — `SELECT ... FROM <undefined table>` (2).** Engine-rejected.

```
organization/@functions.surql:35   FROM has_billing_module
organization/@functions.surql:89   FROM organization_billing_limit
```

On 3.0.5:

```
SELECT * FROM totally_undefined_table;
-> "The table 'totally_undefined_table' does not exist"
```

So `fn::organization::modules`, `fn::organization::modules::contain`,
`fn::organization::modules::require` and `fn::organization::billing::usage`
cannot execute at all. These two are the highest-value E1001s in the baseline.

---

## G5 — guards that are dead code (3 × E1002, 2 × E2004, 1 × W7005, genuine)

Six independent sites where a condition can never be satisfied. Each is a
security or integrity guard, which is what makes them worth reporting.

**G5.1 — `member_of` has no `status`** (`auth/entity_access.surql:15`, E1002).
`member_of` (`core/team/member_of.surql`) is SCHEMAFULL with `role`, `joined_at`
and the relation's `in`/`out`. Both team-membership branches of the
`entity_access` permission clause filter `AND status = 'active'`, which never
matches — the SELECT branch never grants team access, and the CREATE branch's
"prevent granting access to your own teams" protection never fires. Confirmed
against 3.0.5: `SELECT VALUE role FROM m WHERE status = 'active'` on a SCHEMAFULL
table without `status` returns `[]`.

*(Only one of the two sites is reported. The second, at line 36, is suppressed by
BUG-2 below.)*

**G5.2 — `organization_unit` has no `random`** (`organization/organization.surql:78`,
E1002). A leftover `random = 'test'` inside `setup_roles`. Engine-verified: this
is a hard failure on SCHEMAFULL (`Found field 'random', but no such field exists`),
so **every organization CREATE throws**.

**G5.3 — `organization` has no `slug`** (`person/@function.surql:24`, E1002).
`fn::account::username::validate` checks
`SELECT id FROM ONLY organization WHERE slug = $username LIMIT 1`. `organization`
is SCHEMAFULL with 16 fields, none named `slug`. The org-slug half of the
username-uniqueness check is dead — usernames can collide with organization
handles.

**G5.4 — `array::matches(...) = false`** (`organization/@functions.surql:80`, E2004).
`fn::organization::modules::contain` returns `array::matches($modules, $module_key)`.
Engine-verified: `array::matches([1,2,3], 2)` → `[false, true, false]`, i.e.
`array<bool>`. `IF <array<bool>> = false` is never true, so
`fn::organization::modules::require` never THROWs — **the module entitlement gate
never denies anything**. (It is moot in practice because G4b already makes the
function un-runnable, but the two are independent defects.)

**G5.5 — `$admins = 0`** (`organization/employee_of.surql:86`, E2004).
`$admins = SELECT count() FROM employee_of ... GROUP ALL`. Engine-verified:
`SELECT count() FROM t GROUP ALL` → `[{ count: 1 }]`, an array. `IF $admins = 0`
is never true, so `guard_last_admin` never prevents removal of the last
organization administrator. The fix is `$admins[0].count = 0` (and it must also
handle the empty-result `[]` case).

**G5.6 — `$grant.unit != NONE`** (`@functions.surql:23`, W7005).
`employee_of.unit` is declared `TYPE record<organization_unit>` — required, not
`option<>` (`employee_of.surql:19-21`), and `organization.surql:88` always
supplies it on RELATE. So the NONE-guard is unreachable-by-construction. Either
the field should become `option<record<organization_unit>>` (which is what the
guard implies the author believes) or the guard should be deleted. The analyzer
is reading the schema correctly.

---

## G6 — `option<string>` into a `string` parameter (1 × E5002, genuine)

`organization/billing/organization_billing.surql:107-109`:

```surql
DEFINE EVENT OVERWRITE delete_stripe_customer ON organization_billing WHEN $event = 'DELETE' THEN {
	fn::stripe::customer::delete($before.stripe);
};
```

`stripe` is `TYPE option<string>` (line 12-13); `fn::stripe::customer::delete`
declares `$stripe_id: string` (`plugins/stripe.surql:20`). Engine-verified:

```
DEFINE FUNCTION fn::f($x: string) { RETURN $x; };
RETURN fn::f(NONE);
-> "Incorrect arguments for function fn::f(). Failed to coerce argument `$x`:
    Expected `string` but found `NONE`"
```

Deleting an `organization_billing` row that never reached Stripe throws. The fix
is `IF $before.stripe != NONE { … }` or an `option<string>` parameter.

---

## BUG-1 — W4023 fires where `count()` is consumed as an existence test (2 findings)

**Verdict: `BUG` — false positive. Resolves 2 baseline findings.**

Both sites, `schema/@functions.surql:103` and `:113` inside
`fn::entity::permissible`:

```surql
IF array::is_empty(SELECT count() FROM entity_access
	WHERE in IN $teams AND out = $entity AND access_level IN $levels) = false {
	RETURN true;
};
```

The code is **correct as written**. The result is never read as a total — only
its emptiness is. And engine-verified, the diagnostic's suggested remedy is
*actively harmful* in this consumer position:

```
SELECT count() FROM ea WHERE <no match>            -> []              is_empty -> true
SELECT count() FROM ea WHERE <no match> GROUP ALL  -> [{ count: 0 }]  is_empty -> false
```

Taking the advice ("add GROUP ALL for a total") makes `array::is_empty(...) = false`
true for *every* caller, turning `fn::entity::permissible` — the corpus's central
ACL function — into an unconditional `RETURN true`. Emitting a warning whose fix
introduces a privilege escalation is the clearest possible signal that the
contract is stated too broadly.

### Root cause

`crates/workspace/src/analyzer/data/select.rs:1000` —

```rust
fn check_count_without_group(stmt: &ast::SelectStmt, ctx: &mut AnalysisContext<'_>) {
    if stmt.group.is_some() {
        return;
    }
    for projection in &stmt.projections {
        …
        if is_bare_count(call) { ctx.emit(… 4023 …); }
    }
}
```

The check runs per-SELECT with no view of the enclosing expression. Its own doc
comment states the real contract — *"never the row total the author intended…
a guard built on the result (`IF $rows = 0 { THROW … }`) then silently never
fires"* — but the predicate implemented is merely "bare `count()` and no GROUP
clause". The missing half is **how the SELECT's value is consumed**.

### The fix

The contract to implement: fire only when the ungrouped `count()` result reaches
a *scalar-total* position. Suppress when it reaches an *emptiness/cardinality*
position, where per-row `count()` and `GROUP ALL` are not interchangeable.

Because the consumer is one AST level above the SELECT, the predicate cannot
live inside `check_count_without_group` as written. Two options:

1. **Preferred, and a natural fit for the in-flight expression-fact layer.**
   Record the ungrouped-bare-`count()` SELECT as a *fact* rather than emitting
   immediately, and let the expression walker decide at the consumer:
   - consumer is `array::is_empty(_)`, `array::len(_)`, `array::is_empty(_) = <bool>`,
     `count(_)`, or an `IN`/membership position → **suppress**;
   - consumer is a numeric comparison against the value itself
     (`= 0`, `> $n`, arithmetic) or a `VALUE`/`RETURN` of the whole select
     → **emit**;
   - no identifiable consumer → emit (status quo, keeps the check honest).
2. **Minimal, if the fact layer is not ready.** Thread an
   `is_emptiness_consumer: bool` down from the call-expression walker that
   lowers `array::is_empty` / `array::len` arguments, and pass it into
   `check_count_without_group` as a second parameter (`if emptiness_consumer { return; }`).

Do **not** simply downgrade or delete W4023 — the ungrouped-`count()`-as-total
mistake it catches is real (`crates/workspace/src/analysis.rs:2115`
`ungrouped_bare_count_fires_4023_not_5001` covers exactly that case and must keep
passing).

### Regression test

Add to `crates/workspace/src/analyzer/data/select.rs` tests, next to the existing
4023 cases (~line 4100):

```rust
#[test]
fn ungrouped_count_consumed_by_is_empty_does_not_fire_4023() {
    // `array::is_empty(SELECT count() ...)` is an existence test, not a total.
    // Engine-verified: with no matching rows the ungrouped form yields `[]`
    // while `GROUP ALL` yields `[{count: 0}]`, so the two are NOT
    // interchangeable here and the suggested remedy inverts the guard.
    let query = "RETURN array::is_empty(SELECT count() FROM person WHERE age > 18) = false;";
    assert!(!codes(&diagnostics).contains(&4023));
}

#[test]
fn ungrouped_count_compared_to_a_number_still_fires_4023() {
    let query = "LET $n = (SELECT count() FROM person); IF $n = 0 { THROW 'none' };";
    assert!(codes(&diagnostics).contains(&4023));
}
```

Also add an oracle-anchored case so the corpus shape itself is covered:
`array::is_empty(SELECT count() FROM entity_access WHERE in IN $teams) = false`.

---

## BUG-2 — a `--` comment inside an expression deletes everything before it

**Not a baseline entry** (it causes false *negatives*), found while triaging G5.1.
Worth fixing in the same cycle: it silently hides 3 findings in this corpus today
and is a correctness hole in the parser, not the analyzer.

### Symptom

```surql
DEFINE TABLE ea2 TYPE RELATION FROM account TO proj
	PERMISSIONS FOR CREATE WHERE
		in IN (SELECT VALUE out FROM member_of WHERE status = 'a')
		-- a comment here
		AND in IN (SELECT VALUE out FROM member_of WHERE status = 'b');
```

Only the `status = 'b'` E1002 is reported. Remove the comment and both fire.
A trailing same-line comment behaves identically (`… 'g' -- note` / newline /
`AND … 'h'` loses `'g'`). A *leading* comment is harmless. Confirmed in
`PERMISSIONS` clauses and `DEFINE FIELD … ASSERT` expressions; block bodies
(`DEFINE FUNCTION { … }`, `DEFINE EVENT … THEN { … }`) are unaffected.

### Root cause — confirmed against the CST

`cargo run -p surrealguard-syntax --example dump_cst` on the shape above:

```
WhereClause "WHERE x = 1\n\t-- note\n\tAND y = 2"
  Keyword "WHERE"
  BinaryExpression "x = 1\n\t-- note\n\tAND y = 2"
    BinaryExpression "x = 1"      <-- lhs
    Comment "-- note"
    Operator "AND"
    BinaryExpression "y = 2"      <-- rhs
```

The grammar is correct: the comment is a named sibling *between* the lhs and the
operator. The lowering is not.

`crates/syntax/src/lower/expr.rs:209-231`, `fn binary`:

```rust
let lhs = children[..op_index]
    .iter()
    .rev()
    .find(|c| c.kind() != "Operator");        // <-- picks Comment, not the lhs
let rhs = children[op_index + 1..]
    .iter()
    .find(|c| c.kind() != "Operator");
```

With children `[BinaryExpression(x = 1), Comment, Operator(AND), BinaryExpression(y = 2)]`,
`op_index == 2` and the reverse scan for the lhs hits `Comment` first. The real
left operand is discarded and lowered as an opaque node, so the whole left side of
the expression — arbitrarily large — vanishes from analysis.

### The fix

`crates/syntax/src/lower/expr.rs:210`, filter comments out of the child list up
front so every index-sensitive step below is immune:

```rust
let children: Vec<_> = named_children(node)
    .into_iter()
    .filter(|c| !matches!(c.kind(), "Comment" | "BlockComment"))
    .collect();
```

Two sibling sites have the same latent defect and should be hardened in the same
change:

- `crates/syntax/src/lower/expr.rs:233-245`, `fn prefix` —
  `children.iter().find(|c| c.kind() != "Operator")` picks a `Comment` for
  `! -- why` / newline / `x`.
- `crates/syntax/src/lower/statement.rs:426-431`, `fn clause_expr` —
  `.rfind(|child| child.kind() != "Keyword")` picks a trailing `Comment` when one
  is the last child of a `WhereClause` / `LIMIT` clause. Extend to
  `!matches!(child.kind(), "Keyword" | "Comment" | "BlockComment")`.

Note `subquery_content` (expr.rs:681) already filters `"Comment" | "BlockComment"`
— that is the established convention in this file; `binary`, `prefix` and
`clause_expr` simply never adopted it.

### Impact on this corpus

Stripping all `--`/`//` comments from a scratch copy of the corpus and re-running
`check` yields **45** findings instead of 42. The three recovered ones are all in
`schema/auth/entity_access.surql`, hidden behind the comment at line 39:

```
E1002 auth/entity_access.surql:36  `member_of` has no field `status`         (second half of G5.1)
E5002 auth/entity_access.surql:44  argument 1 to `fn::entity::organization` is a `record<organization>`,
                                   but `$e` is declared `record<project | calendar | outlet>`
W7005 auth/entity_access.surql:41  `!=` between `array<record<employee_of>, 1>` and `none` is always true
```

The E5002 is a genuine new defect in the corpus: the `entity_access` CREATE
permission passes `employee_of.out` (a `record<organization>`) to a function
declared to take `record<calendar | project | task | outlet>`.

### Regression test

`crates/syntax/src/lower/expr.rs` tests, next to
`a_comment_inside_parentheses_does_not_recurse_forever` (~line 1170):

```rust
#[test]
fn a_comment_between_operands_does_not_swallow_the_left_side() {
    // The CST puts `Comment` between the lhs and the `Operator`; the reverse
    // scan for the lhs must skip it or the entire left operand is discarded.
    let parsed = parse("SELECT * FROM t WHERE a = 1\n  -- why\n  AND b = 2;");
    let lowered = lower_first(&parsed, "BinaryExpression");
    // lhs must be the `a = 1` comparison, not a Partial
    assert!(matches!(lowered, Expr::Binary { lhs, .. } if matches!(&lhs.node, Expr::Binary { .. })));
}
```

Plus a workspace-level test that the E1002 in a commented permission clause still
fires (the minimal `ea1`/`ea2` pair above makes a good fixture).

---

## Ranking

| Rank | Fix | Baseline findings resolved | Also |
|---|---|---:|---|
| 1 | **BUG-1** — W4023 consumer-awareness (`select.rs:1000`) | 2 | removes a warning whose remedy is a privilege escalation |
| 2 | **BUG-2** — comment-aware `binary` lowering (`expr.rs:210`) | 0 | *un-hides* 3 findings here; a whole-expression correctness hole everywhere |

BUG-2 resolves no baseline entries but is the more serious defect: it is
unbounded (any expression, any file) and silent. BUG-1 is ranked first only
because it is the one item the baseline itself is waiting on.

---

## Findings I could not fully resolve

Nothing in the 42 is unclassified — every verdict above is backed either by a
grep over the whole corpus or by an engine transcript. Three residual
uncertainties are recorded honestly:

1. **W4023's verdict is a product decision, not a fact.** `BUG` is defensible
   (the code is correct and the suggested remedy breaks it) and so is `expected`
   (`array::is_empty(SELECT count() …)` is an odd idiom a reviewer would also
   flag; the plain `SELECT id … LIMIT 1` is clearer). I chose `BUG` because the
   diagnostic's *message* is unambiguously wrong for this input — no total is
   wanted, and following it changes behaviour for the worse. If the author
   prefers to keep the lint broad, the honest alternative is to reword it
   ("`count()` here yields 1 per row; use `GROUP ALL` for a total or
   `SELECT id … LIMIT 1` for existence") and re-verdict these two as `expected`.
   Nothing else in the baseline hinges on the choice.

2. **G5.6 (W7005) assumes the declared schema is the deployed schema.**
   `employee_of.unit` is non-optional *today*, which is what makes
   `$grant.unit != NONE` dead. If rows predate the field definition they could
   still be missing it, and the guard would be defensive rather than dead. I
   graded it `genuine` because a static analyzer can only reason from the
   declarations it is given, and because the corpus's own RELATE always supplies
   `unit`. Resolving it beyond doubt would need the production data.

3. **BUG-2's blast radius is characterised, not bounded.** I confirmed the
   mechanism from the CST and confirmed three concrete recoveries in this corpus,
   but "any binary expression containing a `--` comment" is a large surface and
   other corpora will lose different findings. The comment-stripped re-run
   (42 → 45) is a lower bound for this corpus only.

---

## Reproduction notes

- Engine: `surreal start --user root --pass root --bind 127.0.0.1:18234 memory`
  (SurrealDB 3.0.5, macOS aarch64), stopped after use.
- Analyzer: `target/release/surrealguard` at HEAD `56dbea3`, run with
  `cwd = /Users/drewridley/Documents/Projects/workshop/database`.
- The corpus was never modified. The comment-stripping experiment ran against a
  copy in the session scratchpad.
