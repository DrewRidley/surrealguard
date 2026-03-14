# Analyzer Gap Analysis

Status as of 2026-03-13. 450+ tests passing, 0 failures, 0 ignored.

## Legend
- [x] Implemented and tested
- [~] Implemented but incomplete or untested
- [ ] Not implemented

---

## 1. SELECT

### Implemented
- [x] Table existence (strict mode)
- [x] Field existence on schemafull tables
- [x] SELECT * returns `array<{fields...}>`
- [x] SELECT VALUE returns `array<type>`
- [x] SELECT ONLY returns unwrapped type
- [x] OMIT removes fields (single and multiple)
- [x] OMIT field existence check
- [x] FETCH expands `record<T>` to full schema
- [x] FETCH field must be `record<>` type
- [x] Graph edge validation (multi-hop, relation constraints)
- [x] Graph edge WHERE clause type checking
- [x] Filter `[WHERE ...]` in path elements
- [x] WHERE clause bool type check
- [x] Aliases (`AS name`) in result type
- [x] LIMIT/START must be numeric
- [x] TIMEOUT must be duration
- [x] SPLIT field must be array
- [x] Duplicate field name detection
- [x] Subquery type propagation
- [x] Block result type propagation
- [x] ORDER BY field existence on schemafull tables
- [x] GROUP BY field existence on schemafull tables
- [x] GROUP BY without aggregate functions warning (SG222)
- [x] SPLIT field existence on table
- [x] WITH INDEX hint — referenced index exists (SG005)
- [x] SELECT VALUE with multiple fields warning (SG227)

---

## 2. CREATE

### Implemented
- [x] Table existence (strict mode)
- [x] SET field existence, readonly, computed, type checks
- [x] Assignment operators (+= -= +?=) type validation
- [x] Nested field path validation
- [x] Missing required fields detection
- [x] CONTENT $param → infer full table type
- [x] CONTENT { field: $param } → infer per-field param types
- [x] CONTENT object field existence + value type checking
- [x] CONTENT non-object value type check (SG218)
- [x] CREATE returns `array<{table_schema}>` result type
- [x] CREATE ONLY returns single record
- [x] RETURN NONE/BEFORE/AFTER/DIFF/field projection
- [x] RETURN BEFORE invalid on CREATE warning (SG220)
- [x] Duplicate field assignments detection

---

## 3. UPDATE / UPSERT

### Implemented
- [x] Table existence (strict mode)
- [x] SET field existence, readonly, computed, type checks
- [x] Assignment operators (+= -= +?=) type validation
- [x] Nested field path validation
- [x] WHERE clause bool type check
- [x] CONTENT/MERGE/PATCH/REPLACE $param inference
- [x] CONTENT object field existence + value type checking
- [x] CONTENT non-object value type check
- [x] Returns `array<{table_schema}>`, ONLY returns single
- [x] RETURN NONE/BEFORE/AFTER/DIFF/field projection
- [x] Duplicate field assignments detection
- [x] PATCH (JSON Patch) syntax/structure validation (SG229)

---

## 4. INSERT

### Implemented
- [x] Table existence (strict mode)
- [x] SET field existence/type/readonly checks
- [x] Assignment operators type validation
- [x] Missing required fields detection
- [x] CONTENT $param inference + object param inference
- [x] INSERT returns `array<{table_schema}>`
- [x] RETURN clause type inference
- [x] Duplicate field assignments detection
- [x] VALUES clause: column count, existence, type validation
- [x] INSERT RELATION validation (table must be relation)
- [x] ON DUPLICATE KEY field existence and type validation

---

## 5. RELATE

### Implemented
- [x] Relation table existence (strict mode)
- [x] Table is RELATION type check
- [x] FROM/TO table constraint validation
- [x] SET field existence, readonly, type checks
- [x] Assignment operators type validation
- [x] CONTENT clause param inference
- [x] `in`/`out` auto-field readonly check
- [x] RELATE returns `array<{relation_schema}>`
- [x] RETURN clause type inference
- [x] Duplicate field assignments detection

---

## 6. DELETE

### Implemented
- [x] Table existence (strict mode)
- [x] WHERE clause bool type check
- [x] DELETE returns `array<{table_schema}>`, ONLY returns single
- [x] RETURN NONE/BEFORE/AFTER/DIFF/field projection
- [x] RETURN field existence validation on table

---

## 7. DEFINE Statements

### Implemented
- [x] DEFINE TABLE: name, schema_mode, relation kind, drop flag
- [x] DEFINE FIELD: name, table, type, readonly, computed, flexible, has_default, has_assert
- [x] DEFINE FIELD: DEFAULT expression type checking (SG215)
- [x] DEFINE FIELD: VALUE expression type checking (SG216)
- [x] DEFINE FIELD: REFERENCE clause validation (must be record type)
- [x] DEFINE INDEX: name, table, fields, unique
- [x] DEFINE FUNCTION: name, params with types, return type, body analysis
- [x] DEFINE FUNCTION: return type validation against body (SG106)
- [x] DEFINE EVENT: scope setup with $event/$before/$after/$value, WHEN/THEN analysis
- [x] DEFINE PARAM: param binding with type inference
- [x] DEFINE SCOPE: SIGNIN/SIGNUP query analysis
- [x] Duplicate table/field detection
- [x] Field on non-existent table (strict mode)
- [x] Index on non-existent fields (schemafull)
- [x] record<unknown_table> reference validation (strict)
- [x] ASSERT expression boolean validation
- [x] DEFAULT clause extraction (affects required field detection)
- [x] All DEFINE variants dispatched
- [x] Nested field without parent detection (SG223)
- [x] DEFINE TABLE PERMISSIONS analysis
- [x] DEFINE FIELD PERMISSIONS analysis
- [x] DEFINE TABLE VIEW AS query analysis
- [x] DEFINE TABLE CHANGEFEED validation

---

## 8. Expression Resolver

### Implemented
- [x] All literal types resolved correctly
- [x] Variable/parameter lookup with UndefinedVariable error
- [x] Field access on records with FieldNotFound
- [x] Record IDs → `record<table>` + table existence check (strict)
- [x] Array/object literal type construction
- [x] Object literal duplicate field detection (SG217)
- [x] Binary arithmetic with numeric coercion
- [x] Comparison operator type validation
- [x] CONTAINS/INSIDE operator type validation
- [x] MATCHES operator type validation
- [x] Logical AND/OR operand bool validation
- [x] Unary operators (NOT → bool, negation → preserves type)
- [x] Cast validation via `is_valid_cast`
- [x] Unknown cast target type warning (SG226)
- [x] Custom function: existence + arg count validation
- [x] Built-in function: arg count + arg type validation
- [x] Multi-hop graph validation
- [x] LET binding with shadowing warning
- [x] FOR loop iterator variable type inference + iterable check
- [x] IF/ternary type unification across branches + condition bool check
- [x] Subquery type inference (all DML)
- [x] Block returns last expression type
- [x] Method calls on string/array/object (partial)
- [x] Path expression chain resolution
- [x] Null coalescing unwraps Option
- [x] Closure body type resolution
- [x] Closure parameter type inference from array context
- [x] map/filter/find return type from closure
- [x] Closure argument validation (filter expects bool return)
- [x] BREAK/CONTINUE context validation (must be in loop)
- [x] Unreachable code after RETURN/THROW/BREAK (SG219)
- [x] Array indexing with non-integer expression (SG224)
- [x] Optional chaining on non-optional type warning (SG225)
- [x] String function camelCase aliases (startsWith, endsWith, etc.)

---

## 9. Permissions Analyzer

### Implemented
- [x] Locates all permissions_clause nodes
- [x] Sets up $this variable with table type
- [x] Pushes Permissions scope
- [x] Resolves all expressions in WHERE clause
- [x] Permission WHERE clause bool validation
- [x] $auth, $token, $scope, $session variable binding
- [x] FULL/NONE keyword handling and validation
- [x] Conflicting table-level vs field-level permissions warning (SG228)

---

## 10. Type System

### Implemented
- [x] is_assignable: Any ↔ anything, exact match, null → Option
- [x] is_assignable: numeric coercion (int↔float↔decimal↔number)
- [x] is_assignable: Option unwrap, Array/Set covariance, Record table matching
- [x] is_assignable: Literal::Object structural subtyping
- [x] is_assignable: Either/union handling
- [x] is_compound_assignable: += -= +?= validation
- [x] is_valid_cast: comprehensive matrix
- [x] coerce_numeric: Decimal > Float > Number > Int priority
- [x] type_category: type compatibility grouping for comparisons
- [x] Geometry subtype covariance (Geometry<point> assignable to Geometry<any>)
- [x] Range type — assignment rules (Range ↔ Range only)
- [x] Regex type — full type representation with cast rules

---

## 11. Built-in Functions

### Coverage
All 20 namespaces implemented: array, bytes, crypto, duration, encoding, geo, http, math, meta, object, parse, rand, record, search, session, string, time, type, value, vector.

Arg type validation: string::, array::, math::, crypto::, duration::, object::, time::, geo::, encoding::, parse::, http::.
Closure inference: array::map, filter, find, find_index, any, all.

### Implemented
- [x] array::knn, array::unique functions
- [x] type::regex → Kind::Regex
- [x] type::range → Kind::Range
- [x] http::head returns Kind::Object, others return Kind::Any
- [x] http:: URL argument string validation
- [x] String function camelCase aliases

---

## 12. Statement Coverage

### Fully Handled
- [x] SELECT, CREATE, UPDATE, UPSERT, INSERT, DELETE, RELATE
- [x] All 18 DEFINE variants (including SCOPE SIGNIN/SIGNUP)
- [x] IF, FOR, LET, RETURN, THROW, BREAK, CONTINUE
- [x] SLEEP (duration validation)
- [x] BEGIN, COMMIT, CANCEL (transaction control + balance tracking SG221)
- [x] REMOVE (entity existence checks in strict mode)
- [x] ALTER TABLE (schema mode tracking, DROP flag)
- [x] REBUILD INDEX, INFO, USE, OPTION, KILL, SHOW (dispatched)
- [x] LIVE SELECT (full query analysis + returns Kind::Uuid)
- [x] INFO FOR table existence check (strict mode)
- [x] KILL argument UUID type validation
- [x] SHOW CHANGES table existence + SINCE datetime validation

---

## All Gaps Closed

All 75 originally tracked items have been implemented. No remaining gaps.
