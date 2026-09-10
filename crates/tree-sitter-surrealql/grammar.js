/**
 * SurrealQL tree-sitter grammar.
 *
 * Mirrors the lezer-surrealql grammar 1:1 in tree-sitter form so that the
 * parsed tree can be compared against the lezer parser for parity.
 *
 * Naming conventions (intentionally NOT snake_case):
 *   - Visible rules use the same PascalCase names as lezer node types.
 *   - Hidden rules (lowercase passthrough in lezer) use a leading underscore.
 *   - Token-as-node visibility (Keyword, Operator, BraceOpen, …) is achieved by
 *     defining a visible rule with the lezer name and aliasing source tokens
 *     into it at each use site via alias($._tok, $.Name).
 *
 * Reference: codemirror/packages/lezer-surrealql/src/surrealql.grammar
 */

/// <reference types="tree-sitter-cli/dsl" />
// @ts-check

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/**
 * Case-insensitive keyword regex like /[sS][eE][lL][eE][cC][tT]/. Returned
 * directly (without `token(prec(...))`) so that tree-sitter's lexer applies
 * its default longest-match rules across keyword variants. Identifier vs.
 * keyword disambiguation is handled per-rule via static precedence on
 * `_rawident`.
 * @param {string} word
 */
function kw(word) {
	return new RegExp(
		[...word].map((c) => `[${c.toLowerCase()}${c.toUpperCase()}]`).join(''),
	);
}

/** Comma separated list (no trailing comma) */
function csep(rule) {
	return seq(rule, repeat(seq(',', rule)));
}

/** Comma separated with optional trailing comma */
function csepTrail(rule) {
	return seq(rule, repeat(seq(',', rule)), optional(','));
}

/** Pipe separated */
function piped(rule) {
	return seq(rule, repeat(seq('|', rule)));
}

/** Digit sequence with optional underscore separators (e.g. 1_000_000) */
const DIGITS = /[0-9]+(?:_[0-9]+)*/;

/** Case-insensitive alternation source, for embedding in a larger RegExp. */
function kwAlt(words) {
	return words.map((word) => kw(word).source).join('|');
}

/**
 * The constants SurrealQL resolves as bare paths — `math::PI` is a value, not
 * a zero-argument call. The set is closed: 3.2.3 rejects `math::nan`,
 * `time::MAX` and `duration::MIN` at parse time, so listing them exactly is
 * what keeps the grammar from being looser than the engine.
 */
const MATH_CONSTANTS = [
	'E',
	'FRAC_1_PI',
	'FRAC_1_SQRT_2',
	'FRAC_2_PI',
	'FRAC_2_SQRT_PI',
	'FRAC_PI_2',
	'FRAC_PI_3',
	'FRAC_PI_4',
	'FRAC_PI_6',
	'FRAC_PI_8',
	'INF',
	'INFINITY',
	'LN_10',
	'LN_2',
	'LOG10_2',
	'LOG10_E',
	'LOG2_10',
	'LOG2_E',
	'NEG_INF',
	'NEG_INFINITY',
	'PI',
	'SQRT_2',
	'TAU',
];

/**
 * Every keyword token, by the suffix of its `_kw_*` rule. `_any_kw` is the
 * union of them and `_fetchKeywordRoot` aliases each one to `Keyword`.
 */
const KEYWORDS = [
	'access',
	'algorithm',
	'all',
	'alpha',
	'alter',
	'always',
	'analyzer',
	'and',
	'any',
	'api',
	'as',
	'asc',
	'assert',
	'at',
	'async',
	'authenticate',
	'auto',
	'backend',
	'begin',
	'bm25',
	'break',
	'bucket',
	'by',
	'cancel',
	'capacity',
	'cascade',
	'changefeed',
	'changes',
	'collate',
	'columns',
	'comment',
	'commit',
	'computed',
	'concurrently',
	'config',
	'content',
	'continue',
	'create',
	'database',
	'db',
	'default',
	'defer',
	'define',
	'delete',
	'desc',
	'dimension',
	'dist',
	'doc_ids_cache',
	'doc_ids_order',
	'doc_lengths_cache',
	'doc_lengths_order',
	'drop',
	'duplicate',
	'duration',
	'efc',
	'else',
	'end',
	'enforced',
	'event',
	'exclude',
	'exists',
	'explain',
	'expunge',
	'extend_candidates',
	'fetch',
	'field',
	'fields',
	'filters',
	'flexible',
	'for',
	'from',
	'function',
	'functions',
	'get',
	'graphql',
	'group',
	'highlights',
	'hnsw',
	'diskann',
	'degree',
	'l_build',
	'hashed_vector',
	'if',
	'ignore',
	'in',
	'include',
	'index',
	'info',
	'insert',
	'into',
	'is',
	'issuer',
	'jwt',
	'keep_pruned_connections',
	'key',
	'kill',
	'let',
	'limit',
	'live',
	'lm',
	'm',
	'm0',
	'merge',
	'middleware',
	'maxdepth',
	'mtree',
	'mtree_cache',
	'namespace',
	'noindex',
	'normal',
	'not',
	'ns',
	'numeric',
	'omit',
	'on',
	'only',
	'option',
	'or',
	'order',
	'out',
	'overwrite',
	'parallel',
	'param',
	'passhash',
	'password',
	'patch',
	'permissions',
	'post',
	'postings_cache',
	'postings_order',
	'prepare',
	'put',
	'readonly',
	'rebuild',
	'record',
	'reference',
	'reject',
	'relate',
	'relation',
	'remove',
	'replace',
	'retry',
	'return',
	'roles',
	'root',
	'sc',
	'schemafull',
	'schemaless',
	'scope',
	'search',
	'select',
	'session',
	'set',
	'show',
	'signin',
	'signup',
	'since',
	'sleep',
	'split',
	'start',
	'strict',
	'structure',
	'table',
	'tables',
	'tb',
	'tempfiles',
	'terms_cache',
	'terms_order',
	'then',
	'throw',
	'timeout',
	'to',
	'token',
	'tokenizers',
	'trace',
	'transaction',
	'type',
	'unique',
	'unset',
	'update',
	'upsert',
	'url',
	'use',
	'user',
	'value',
	'values',
	'version',
	'when',
	'where',
	'with',
];

// ---------------------------------------------------------------------------
// Grammar
// ---------------------------------------------------------------------------

export default grammar({
	name: 'surrealql',

	word: ($) => $._rawident,

	extras: ($) => [/\s/, $.Comment, $.BlockComment],

	externals: ($) => [
		$._js_function_body,
		$._object_open, // emitted when '{' starts an Object (vs Block/Set)
	],

	precedences: ($) => [
		[
			'prefix',
			'range',
			// A cast binds looser than a range (`<array> 1..5` is `[1, 2, 3, 4]`
			// on 3.2.3) and tighter than every binary operator (`<string> 1 + 2`
			// is a string + int error).
			'cast',
			'method',
			// Binary operator tiers, tightest to loosest. Splitting the former
			// single 'binary' level is what makes `a > 1 AND b > 2` parse as
			// `(a > 1) AND (b > 2)` instead of flat-left. The ordering mirrors
			// surrealdb-core's `BindingPower` enum
			// (Nullish < Or < And < Equality < Relation < AddSub < MulDiv <
			// Power), so the nesting matches how the engine evaluates.
			'binary_power',
			'binary_multiplicative',
			'binary_additive',
			'binary_relation',
			'binary_equality',
			'binary_conjunction',
			'binary_disjunction',
			'binary_nullish',
			'closure',
			'union',
			'filter',
			'for',
			'clause',
		],
	],

	conflicts: ($) => [
		[$.RecordId, $.RecordIdRange],
		[$.Path],
		[$.Path, $.Destructure],
		[$.WhereClause],
		[$._baseValue, $.Closure],
		[$._idName, $._singleType],
		// The dangling ELSE of the THEN…END form. A branch body is a value and
		// a value can be another IF, so `ELSE IF` either continues this chain
		// or opens a nested one; both are real readings, settled dynamically.
		[$.Legacy],
		[$._prefixOperand, $.Path],
		[$._value, $.Path],
		// A bare RETURN/THROW as an IF-THEN body can contain a block-form
		// (Modern) IF, whose ELSE chain is a dangling-else ambiguity; let the
		// GLR parser explore both and settle it dynamically.
		[$.Modern],
		// After `WITH JWT <jwt>` inside DEFINE ACCESS ... TYPE RECORD, a `WITH`
		// starts either the JWT clause's own `WITH ISSUER` or the type's
		// trailing `WITH REFRESH`. Deciding needs the token after `WITH`, so
		// the grammar is LR(2) here — not ambiguous. Let GLR look ahead.
		[$.JwtClause],
		// `-5` is a signed literal and `-$x` a prefix negation; both start the
		// same way, and a signed literal is preferred (dynamic precedence on
		// `Number`) whenever the operand is a bare number.
		[$.Number],
		// `not(1, 2)` is a call with two arguments, not `NOT` applied to the
		// point `(1, 2)`; the call carries dynamic precedence.
		[$._computedValue, $.Point],
	],

	rules: {
		// ================================================================
		// Top-level entry
		// ================================================================

		SurrealQL: ($) => optional($._expressions),

		_expressions: ($) =>
			prec.right(
				seq(
					$._expression,
					repeat(seq(';', $._expression)),
					optional(';'),
				),
			),

		_expression: ($) => choice($._statement, $._value),

		// ================================================================
		// Statements
		// ================================================================

		// IfElseStatement is deliberately absent: IF is a value (see `_value`),
		// so listing it here as well would make every `IF …` in an expression
		// position reachable two ways for the same tree.
		_subqueryStatement: ($) =>
			choice(
				$.LetStatement,
				$.DeleteStatement,
				$.CreateStatement,
				$.SelectStatement,
				$.RelateStatement,
				$.UpdateStatement,
				$.RemoveStatement,
				$.UpsertStatement,
				$.ReturnStatement,
				$.AlterStatement,
				$.DefineStatement,
				$.RebuildStatement,
				$.InsertStatement,
			),

		_statement: ($) =>
			choice(
				$.BeginStatement,
				$.CancelStatement,
				$.CommitStatement,
				$.InfoForStatement,
				$.AccessStatement,
				$.KillStatement,
				$.LiveSelectStatement,
				$.ShowStatement,
				$.SleepStatement,
				$.UseStatement,
				$.OptionStatement,
				$.BreakStatement,
				$.ContinueStatement,
				$.ForStatement,
				$.ThrowStatement,
				$._subqueryStatement,
			),

		// ----------------------------------------------------------------
		// Transaction statements
		// ----------------------------------------------------------------

		BeginStatement: ($) =>
			seq(
				alias($._kw_begin, $.Keyword),
				optional(alias($._kw_transaction, $.Keyword)),
			),

		CancelStatement: ($) =>
			seq(
				alias($._kw_cancel, $.Keyword),
				optional(alias($._kw_transaction, $.Keyword)),
			),

		CommitStatement: ($) =>
			seq(
				alias($._kw_commit, $.Keyword),
				optional(alias($._kw_transaction, $.Keyword)),
			),

		// ----------------------------------------------------------------
		// Simple statements
		// ----------------------------------------------------------------

		BreakStatement: ($) => alias($._kw_break, $.Keyword),
		ContinueStatement: ($) => alias($._kw_continue, $.Keyword),
		SleepStatement: ($) => seq(alias($._kw_sleep, $.Keyword), $.Duration),
		ThrowStatement: ($) => seq(alias($._kw_throw, $.Keyword), $._value),
		// RETURN carries its own FETCH clause. It is spelled against `_value`
		// rather than `_expression` so that `RETURN SELECT … FETCH a` gives the
		// FETCH to the SELECT, which already has one, instead of leaving the
		// two rules to fight over it.
		ReturnStatement: ($) =>
			seq(
				alias($._kw_return, $.Keyword),
				choice(
					$._statement,
					seq($._value, optional($.FetchClause)),
				),
			),

		OptionStatement: ($) =>
			seq(
				alias($._kw_option, $.Keyword),
				$.Ident,
				optional(
					seq(
						'=',
						choice(
							alias($._kw_true, $.Bool),
							alias($._kw_false, $.Bool),
						),
					),
				),
			),

		// The live query is named by a UUID literal or a param holding one;
		// 3.2.3 rejects every other literal, `KILL "…"` included.
		KillStatement: ($) =>
			seq(
				alias($._kw_kill, $.Keyword),
				choice(alias($._uuidString, $.String), $.VariableName),
			),

		// USE
		UseStatement: ($) =>
			seq(
				alias($._kw_use, $.Keyword),
				choice($._useNs, $._useDb, seq($._useNs, $._useDb)),
			),
		_useNs: ($) => seq($._nsKeyword, $.Ident),
		_useDb: ($) => seq($._dbKeyword, $.Ident),

		// SHOW
		ShowStatement: ($) =>
			seq(
				alias($._kw_show, $.Keyword),
				alias($._kw_changes, $.Keyword),
				alias($._kw_for, $.Keyword),
				// One table, or every table in the database.
				choice(
					seq(alias($._kw_table, $.Keyword), $.Ident),
					$._dbKeyword,
				),
				// SINCE is not optional — `SHOW CHANGES FOR TABLE t` on its own
				// is a parse error in 3.2.3.
				seq(
					alias($._kw_since, $.Keyword),
					// SINCE accepts a datetime string or a versionstamp number.
					choice($.String, $.Number),
				),
				optional(seq(alias($._kw_limit, $.Keyword), $.Number)),
			),

		// ACCESS — operate on the grants of a DEFINE ACCESS method. The level
		// clause is optional (it defaults to the session's), and each verb
		// carries its own operand shape. The grant id is an Ident and only an
		// Ident: 3.2.3 rejects `SHOW GRANT $g` and `GRANT FOR USER $u`.
		AccessStatement: ($) =>
			seq(
				alias($._kw_access, $.Keyword),
				$.Ident,
				optional($.OnRootNsDbClause),
				choice(
					$.AccessGrantClause,
					$.AccessShowClause,
					$.AccessRevokeClause,
					$.AccessPurgeClause,
				),
			),
		AccessGrantClause: ($) =>
			seq(
				alias($._kw_grant, $.Keyword),
				alias($._kw_for, $.Keyword),
				choice(
					seq(alias($._kw_user, $.Keyword), $.Ident),
					seq(alias($._kw_record, $.Keyword), $.RecordId),
				),
			),
		AccessShowClause: ($) =>
			seq(alias($._kw_show, $.Keyword), $._accessSubject),
		AccessRevokeClause: ($) =>
			seq(alias($._kw_revoke, $.Keyword), $._accessSubject),
		// SHOW and REVOKE select the same way: everything, one grant by id, or
		// a predicate over the grant records.
		_accessSubject: ($) =>
			choice(
				alias($._kw_all, $.Keyword),
				seq(alias($._kw_grant, $.Keyword), $.Ident),
				$.WhereClause,
			),
		AccessPurgeClause: ($) =>
			seq(
				alias($._kw_purge, $.Keyword),
				csep(
					choice(
						alias($._kw_expired, $.Keyword),
						alias($._kw_revoked, $.Keyword),
					),
				),
				optional(
					seq(alias($._kw_for, $.Keyword), $.Duration),
				),
			),

		// INFO FOR
		InfoForStatement: ($) =>
			seq(
				alias($._kw_info, $.Keyword),
				alias($._kw_for, $.Keyword),
				choice(
					alias($._kw_root, $.Keyword),
					$._nsKeyword,
					$._dbKeyword,
					seq(alias($._kw_sc, $.Keyword), $.Ident),
					seq(alias($._kw_scope, $.Keyword), $.Ident),
					seq(alias($._kw_tb, $.Keyword), $.Ident),
					seq(alias($._kw_table, $.Keyword), $.Ident),
					// INFO FOR USER defaults to the session's level when the
					// ON clause is omitted.
					seq(
						alias($._kw_user, $.Keyword),
						$.Ident,
						optional($.OnRootNsDbClause),
					),
					seq(
						alias($._kw_index, $.Keyword),
						$.Ident,
						$.OnTableClause,
					),
				),
				optional(alias($._kw_structure, $.Keyword)),
			),

		// LET
		LetStatement: ($) =>
			seq(
				alias($._kw_let, $.Keyword),
				$.ParamDefinition,
				'=',
				choice($._value, $._subqueryStatement),
			),

		// REBUILD
		RebuildStatement: ($) =>
			seq(
				alias($._kw_rebuild, $.Keyword),
				alias($._kw_index, $.Keyword),
				optional($.IfExistsClause),
				$.Ident,
				$.OnTableClause,
			),

		// FOR
		ForStatement: ($) =>
			seq(
				alias($._kw_for, $.Keyword),
				$.VariableName,
				alias($._kw_in, $.Keyword),
				// Any expression, not just a literal collection: the engine
				// evaluates it and then complains at runtime if the result
				// isn't iterable, so `(SELECT …) * 2` parses.
				choice($._value, $._subqueryStatement),
				$.Block,
			),

		// IF/ELSE
		IfElseStatement: ($) =>
			seq(alias($._kw_if, $.Keyword), choice($.Legacy, $.Modern)),
		// The branch bodies are `_value`, which already covers Block and
		// SubQuery; spelling those out again would make each one reachable two
		// ways for the same tree, which the dangling-ELSE conflict below can no
		// longer absorb.
		Legacy: ($) =>
			seq(
				$._value,
				alias($._kw_then, $.Keyword),
				choice($._value, $.ThrowStatement, $.ReturnStatement),
				optional(';'),
				repeat(
					seq(
						alias($._kw_else, $.Keyword),
						alias($._kw_if, $.Keyword),
						$._value,
						alias($._kw_then, $.Keyword),
						choice($._value, $.ThrowStatement, $.ReturnStatement),
						optional(';'),
					),
				),
				optional(
					seq(
						alias($._kw_else, $.Keyword),
						choice($._value, $.ThrowStatement, $.ReturnStatement),
						optional(';'),
					),
				),
				alias($._kw_end, $.Keyword),
			),
		Modern: ($) =>
			seq(
				$._value,
				$.Block,
				repeat(
					seq(
						alias($._kw_else, $.Keyword),
						alias($._kw_if, $.Keyword),
						$._value,
						$.Block,
					),
				),
				optional(seq(alias($._kw_else, $.Keyword), $.Block)),
			),

		// LIVE SELECT
		LiveSelectStatement: ($) =>
			seq(
				alias($._kw_live, $.Keyword),
				alias($._kw_select, $.Keyword),
				choice(
					alias($._kw_diff, $.Literal),
					seq(alias($._kw_value, $.Keyword), $.Predicate),
					csep($._inclusivePredicate),
				),
				alias($._kw_from, $.Keyword),
				csep(choice($.Ident, $.RecordId, $.VariableName)),
				optional($.WhereClause),
				optional($.FetchClause),
			),

		// ALTER
		AlterStatement: ($) =>
			seq(
				alias($._kw_alter, $.Keyword),
				choice(
					seq(
						alias($._kw_table, $.Keyword),
						optional(choice($.IfNotExistsClause, $.IfExistsClause)),
						$._value,
						repeat(
							choice(
								alias($._kw_drop, $.Keyword),
								alias($._kw_schemafull, $.Keyword),
								alias($._kw_schemaless, $.Keyword),
								$.TableTypeClause,
								$.ChangefeedClause,
								$.PermissionsForClause,
								$.CommentClause,
							),
						),
					),
					// ALTER INDEX names the table it belongs to and needs at
					// least one change: bare `ALTER INDEX i ON t` is a parse
					// error in 3.2.3.
					seq(
						alias($._kw_index, $.Keyword),
						optional($.IfExistsClause),
						$.Ident,
						$.OnTableClause,
						repeat1(
							choice($.PrepareClause, $.CommentClause),
						),
					),
				),
			),
		PrepareClause: ($) =>
			seq(
				alias($._kw_prepare, $.Keyword),
				optional(alias($._kw_remove, $.Keyword)),
			),

		// REMOVE
		RemoveStatement: ($) =>
			seq(
				alias($._kw_remove, $.Keyword),
				choice(
					seq(
						$._nsKeyword,
						optional($.IfExistsClause),
						$._value,
						optional(
							seq(
								alias($._kw_and, $.Keyword),
								alias($._kw_expunge, $.Keyword),
							),
						),
					),
					seq(
						$._dbKeyword,
						optional($.IfExistsClause),
						$._value,
						optional(
							seq(
								alias($._kw_and, $.Keyword),
								alias($._kw_expunge, $.Keyword),
							),
						),
					),
					seq(
						alias($._kw_user, $.Keyword),
						optional($.IfExistsClause),
						$._value,
						$.OnRootNsDbClause,
					),
					seq(
						alias($._kw_access, $.Keyword),
						optional($.IfExistsClause),
						$._value,
						$.OnRootNsDbClause,
					),
					seq(
						alias($._kw_sequence, $.Keyword),
						optional($.IfExistsClause),
						$._value,
					),
					seq(
						alias($._kw_config, $.Keyword),
						optional($.IfExistsClause),
						choice(
							alias($._kw_graphql, $.Keyword),
							alias($._kw_api, $.Keyword),
							alias($._kw_default, $.Keyword),
						),
					),
					seq(
						alias($._kw_token, $.Keyword),
						optional($.IfExistsClause),
						$._value,
						alias($._kw_on, $.Keyword),
						choice(
							$._nsKeyword,
							$._dbKeyword,
							alias($._kw_scope, $.Keyword),
						),
					),
					seq(
						alias($._kw_event, $.Keyword),
						optional($.IfExistsClause),
						$._value,
						$.OnTableClause,
					),
					seq(
						alias($._kw_field, $.Keyword),
						optional($.IfExistsClause),
						$._value,
						$.OnTableClause,
					),
					seq(
						alias($._kw_index, $.Keyword),
						optional($.IfExistsClause),
						$._value,
						$.OnTableClause,
					),
					seq(
						alias($._kw_analyzer, $.Keyword),
						optional($.IfExistsClause),
						$._value,
					),
					seq(
						alias($._kw_function, $.Keyword),
						optional($.IfExistsClause),
						$.FunctionName,
						// The argument list may be written out and is always
						// empty: `REMOVE FUNCTION fn::greet()`.
						optional(seq('(', ')')),
					),
					seq(
						alias($._kw_param, $.Keyword),
						optional($.IfExistsClause),
						$.VariableName,
					),
					seq(
						alias($._kw_scope, $.Keyword),
						optional($.IfExistsClause),
						$._value,
					),
					seq(
						alias($._kw_table, $.Keyword),
						optional($.IfExistsClause),
						$._value,
						optional(
							seq(
								alias($._kw_and, $.Keyword),
								alias($._kw_expunge, $.Keyword),
							),
						),
					),
					seq(
						alias($._kw_api, $.Keyword),
						optional($.IfExistsClause),
						$._value,
					),
					seq(
						alias($._kw_bucket, $.Keyword),
						optional($.IfExistsClause),
						$._value,
					),
				),
			),

		// DEFINE
		DefineStatement: ($) =>
			seq(
				alias($._kw_define, $.Keyword),
				choice(
					$.AccessDefinition,
					seq($._nsKeyword, $._defineNamespaceOptions),
					seq($._dbKeyword, $._defineDatabaseOptions),
					seq(
						alias($._kw_sequence, $.Keyword),
						$._defineSequenceOptions,
					),
					seq(alias($._kw_user, $.Keyword), $._defineUserOptions),
					seq(alias($._kw_token, $.Keyword), $._defineTokenOptions),
					seq(alias($._kw_event, $.Keyword), $._defineEventOptions),
					seq(alias($._kw_field, $.Keyword), $._defineFieldOptions),
					seq(alias($._kw_index, $.Keyword), $._defineIndexOptions),
					seq(
						alias($._kw_analyzer, $.Keyword),
						$._defineAnalyzerOptions,
					),
					seq(
						alias($._kw_function, $.Keyword),
						$._defineFunctionOptions,
					),
					seq(alias($._kw_param, $.Keyword), $._defineParamOptions),
					$.ScopeDefinition,
					seq(alias($._kw_table, $.Keyword), $._defineTableOptions),
					seq(alias($._kw_config, $.Keyword), $._defineConfigOptions),
					seq(alias($._kw_api, $.Keyword), $._defineApiOptions),
					seq(alias($._kw_bucket, $.Keyword), $._defineBucketOptions),
				),
			),
		AccessDefinition: ($) =>
			seq(alias($._kw_access, $.Keyword), $._defineAccessOptions),
		ScopeDefinition: ($) =>
			seq(alias($._kw_scope, $.Keyword), $._defineScopeOptions),

		_defineAccessOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				$.OnRootNsDbClause,
				repeat(
					choice(
						$.AccessTypeClause,
						$.AuthenticateClause,
						$.DurationClause,
						$.CommentClause,
					),
				),
			),

		_defineAnalyzerOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				repeat(
					choice(
						$.TokenizersClause,
						$.FiltersClause,
						$.FunctionClause,
						$.CommentClause,
					),
				),
			),

		_defineEventOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				$.OnTableClause,
				repeat(
					choice(
						$.WhenClause,
						$.ThenClause,
						$.AsyncClause,
						$.CommentClause,
					),
				),
			),

		_defineDatabaseOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				optional(alias($._kw_strict, $.Keyword)),
				optional($.CommentClause),
			),

		_defineFieldOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$.Idiom,
				$.OnTableClause,
				repeat(
					choice(
						$.TypeClause,
						$.DefaultClause,
						$.ReadonlyClause,
						$.ValueClause,
						$.AssertClause,
						$.PermissionsForClause,
						$.CommentClause,
						$.ReferenceClause,
						$.ComputedClause,
						$.RatelimitClause,
					),
				),
			),

		_defineFunctionOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$.FunctionName, // customFunctionName aliased to FunctionName
				seq('(', optional(csepTrail($.ParamDefinition)), ')'),
				optional(seq($.LookupRight, $._type)),
				$.Block,
				repeat(choice($.PermissionsBasicClause, $.CommentClause)),
			),

		_defineIndexOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				$.OnTableClause,
				repeat(
					choice(
						$.FieldsColumnsClause,
						$.IndexClause,
						$.CommentClause,
						$.ConcurrentlyClause,
						$.DeferClause,
					),
				),
			),
		ConcurrentlyClause: ($) => alias($._kw_concurrently, $.Keyword),
		DeferClause: ($) => alias($._kw_defer, $.Keyword),

		_defineNamespaceOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				optional($.CommentClause),
			),

		// DEFINE SEQUENCE takes BATCH/START/TIMEOUT in any order; the engine
		// has no COMMENT on this statement.
		_defineSequenceOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				repeat(
					choice(
						$.SequenceBatchClause,
						$.SequenceStartClause,
						$.TimeoutClause,
					),
				),
			),
		SequenceBatchClause: ($) =>
			seq(alias($._kw_batch, $.Keyword), $.Number),
		SequenceStartClause: ($) =>
			seq(alias($._kw_start, $.Keyword), $.Number),

		_defineParamOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$.VariableName,
				alias($._kw_value, $.Keyword),
				$._value,
				repeat(choice($.PermissionsBasicClause, $.CommentClause)),
			),

		_defineScopeOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				repeat(
					choice(
						$.SessionClause,
						$.SigninClause,
						$.SignupClause,
						$.CommentClause,
					),
				),
			),

		_defineTableOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				repeat(
					choice(
						alias($._kw_drop, $.Keyword),
						alias($._kw_schemafull, $.Keyword),
						alias($._kw_schemaless, $.Keyword),
						$.TableTypeClause,
						$.TableViewClause,
						$.ChangefeedClause,
						$.PermissionsForClause,
						$.RatelimitClause,
						$.CommentClause,
					),
				),
			),

		// RATELIMIT FOR <actions> [WHERE cond] [BY key] LIMIT n PER duration
		// [MAX n], comma-separated per action group.
		RatelimitClause: ($) =>
			seq(alias($._kw_ratelimit, $.Keyword), csep($.RatelimitGroup)),
		RatelimitGroup: ($) =>
			seq(
				alias($._kw_for, $.Keyword),
				csep(
					choice(
						alias($._kw_select, $.Keyword),
						alias($._kw_create, $.Keyword),
						alias($._kw_update, $.Keyword),
						alias($._kw_delete, $.Keyword),
					),
				),
				optional($.WhereClause),
				optional(seq(alias($._kw_by, $.Keyword), $._value)),
				alias($._kw_limit, $.Keyword),
				$.Number,
				alias($._kw_per, $.Keyword),
				$.Duration,
				optional(seq(alias($._kw_max, $.Keyword), $.Number)),
			),

		_defineConfigOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				choice(
					seq(
						alias($._kw_graphql, $.Keyword),
						$._defineConfigGraphqlOptions,
					),
					seq(alias($._kw_api, $.Keyword), $.ApiOptions),
				),
			),
		_defineConfigGraphqlOptions: ($) =>
			repeat1(
				choice(
					alias($._kw_none, $.None),
					alias($._kw_auto, $.Keyword),
					seq(
						alias($._kw_tables, $.Keyword),
						choice(
							alias($._kw_none, $.None),
							alias($._kw_auto, $.Keyword),
							seq(alias($._kw_include, $.Keyword), csep($.Ident)),
							seq(alias($._kw_exclude, $.Keyword), csep($.Ident)),
						),
					),
					seq(
						alias($._kw_functions, $.Keyword),
						choice(
							alias($._kw_none, $.None),
							alias($._kw_auto, $.Keyword),
						),
					),
				),
			),

		_defineTokenOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				seq(
					alias($._kw_on, $.Keyword),
					choice(
						$._nsKeyword,
						$._dbKeyword,
						seq(alias($._kw_scope, $.Keyword), $._value),
					),
				),
				$.TokenTypeClause,
				seq(alias($._kw_value, $.Keyword), $.String),
			),

		// The engine takes DEFINE USER's optional clauses in any order, and
		// each one may be repeated (the last wins). `repeat(choice(...))` is
		// the honest shape; a fixed `seq` rejects `COMMENT 'x' PASSWORD 'y'`.
		_defineUserOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				$.OnRootNsDbClause,
				repeat(
					choice(
						$.PasswordClause,
						$.RolesClause,
						$.DurationClause,
						$.CommentClause,
					),
				),
			),
		PasswordClause: ($) =>
			seq(
				choice(
					alias($._kw_password, $.Keyword),
					alias($._kw_passhash, $.Keyword),
				),
				$.String,
			),
		RolesClause: ($) => seq(alias($._kw_roles, $.Keyword), csep($.Ident)),

		_defineApiOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$.String,
				optional($.ApiOptions),
				repeat1(
					seq(
						alias($._kw_for, $.Keyword),
						choice(alias($._kw_any, $.Keyword), csep($.HttpMethod)),
						optional($.ApiOptions),
						alias($._kw_then, $.Keyword),
						$.Block,
					),
				),
			),

		ApiOptions: ($) =>
			repeat1(choice($.PermissionsBasicClause, $.MiddlewareClause)),

		_defineBucketOptions: ($) =>
			seq(
				optional(choice($.IfNotExistsClause, $.OverwriteClause)),
				$._value,
				repeat(
					choice(
						$.BackendClause,
						$.PermissionsBasicClause,
						$.CommentClause,
					),
				),
			),

		// CREATE
		CreateStatement: ($) =>
			seq(
				alias($._kw_create, $.Keyword),
				optional(alias($._kw_only, $.Keyword)),
				csep(
					choice(
						$.Ident,
						$.VariableName,
						$.FunctionCall,
						$.RecordId,
						$.RangeRecordId,
					),
				),
				optional(choice($.ContentClause, $.SetClause, $.UnsetClause)),
				optional($.ReturnClause),
				optional($.TimeoutClause),
				optional($.ParallelClause),
			),

		// SELECT
		SelectStatement: ($) =>
			seq(
				// `EXPLAIN SELECT …` — the prefix spelling (3.2.3 accepts it
				// bare; `EXPLAIN FULL SELECT` is a parse error there, so FULL
				// stays a trailing-clause-only option).
				optional(alias($._kw_explain, $.Keyword)),
				alias($._kw_select, $.Keyword),
				$.Fields,
				optional($.OmitClause),
				alias($._kw_from, $.Keyword),
				optional(alias($._kw_only, $.Keyword)),
				choice(
					$._statement,
					seq(csep($._value), repeat($._modifierClause)),
				),
			),

		// DELETE
		DeleteStatement: ($) =>
			seq(
				alias($._kw_delete, $.Keyword),
				// `DELETE FROM t` is the same statement as `DELETE t`.
				optional(alias($._kw_from, $.Keyword)),
				optional(alias($._kw_only, $.Keyword)),
				choice(
					$._statement,
					seq(
						csep($._value),
						repeat(
							choice(
								$.WithClause,
								$.WhereClause,
								$.ReturnClause,
								$.TimeoutClause,
								$.ParallelClause,
								$.ExplainClause,
							),
						),
					),
				),
			),

		// INSERT
		InsertStatement: ($) =>
			seq(
				alias($._kw_insert, $.Keyword),
				optional(alias($._kw_ignore, $.Keyword)),
				optional(alias($._kw_relation, $.Keyword)),
				optional(
					seq(
						alias($._kw_into, $.Keyword),
						choice($.Ident, $.VariableName),
					),
				),
				choice(
					$.Object,
					$.VariableName,
					$.BulkInsert,
					// The rows can come from a subquery: `INSERT INTO t
					// (SELECT … FROM u)`. Spelled as the statement rather than
					// `SubQuery` so it stays distinct from the parenthesised
					// column list below, which a general value would make
					// ambiguous until the token after the closing paren. The
					// alias sits on a named rule: aliasing a bare `seq` renames
					// each of its members instead of wrapping them.
					alias($._insertSubquery, $.SubQuery),
					seq(
						'(',
						csep($.Ident),
						')',
						alias($._kw_values, $.Keyword),
						csep(seq('(', csep($._value), ')')),
					),
				),
				optional(
					seq(
						alias($._kw_on, $.Keyword),
						alias($._kw_duplicate, $.Keyword),
						alias($._kw_key, $.Keyword),
						alias($._kw_update, $.Keyword),
						csep($.FieldAssignment),
					),
				),
				optional($.ReturnClause),
			),
		_insertSubquery: ($) => seq('(', $._subqueryStatement, ')'),
		BulkInsert: ($) => seq('[', csep($.Object), ']'),

		// UPDATE
		UpdateStatement: ($) =>
			seq(
				alias($._kw_update, $.Keyword),
				optional(alias($._kw_only, $.Keyword)),
				choice(
					$._statement,
					seq(
						csep($._value),
						optional($.WithClause),
						optional($._dataClause),
						optional($.WhereClause),
						optional($.ReturnClause),
						optional($.TimeoutClause),
						optional($.ParallelClause),
						optional($.ExplainClause),
					),
				),
			),

		// UPSERT
		UpsertStatement: ($) =>
			seq(
				alias($._kw_upsert, $.Keyword),
				optional(alias($._kw_only, $.Keyword)),
				choice(
					$._statement,
					seq(
						csep($._value),
						optional($.WithClause),
						optional($._dataClause),
						optional($.WhereClause),
						optional($.ReturnClause),
						optional($.TimeoutClause),
						optional($.ParallelClause),
						optional($.ExplainClause),
					),
				),
			),

		// RELATE
		// Any end of the edge may be produced by a subquery:
		// `RELATE [1,2]->a:b->(CREATE foo)`.
		_relateSubject: ($) =>
			choice(
				$.Array,
				$.Ident,
				$.FunctionCall,
				$.VariableName,
				$.RecordId,
				$.SubQuery,
			),
		RelateStatement: ($) =>
			seq(
				alias($._kw_relate, $.Keyword),
				optional(alias($._kw_only, $.Keyword)),
				$._relateSubject,
				choice($.LookupRight, $.LookupLeft),
				$._relateSubject,
				choice($.LookupRight, $.LookupLeft),
				$._relateSubject,
				// UNIQUE belongs to the edge, so it sits with the subject and
				// ahead of the data: 3.2.3 rejects `SET x = 1 UNIQUE`.
				optional($.UniqueClause),
				optional(choice($.ContentClause, $.SetClause)),
				optional($.ReturnClause),
				optional($.TimeoutClause),
				optional($.ParallelClause),
			),

		// ----------------------------------------------------------------
		// Modifier / data clauses
		// ----------------------------------------------------------------

		_modifierClause: ($) =>
			choice(
				$.WithClause,
				$.WhereClause,
				$.SplitClause,
				$.GroupClause,
				$.OrderClause,
				$.LimitStartComboClause,
				$.FetchClause,
				$.TimeoutClause,
				$.ParallelClause,
				$.TempfilesClause,
				$.ExplainClause,
				$.VersionClause,
				$.ReturnClause,
			),

		_dataClause: ($) =>
			choice(
				$.ContentClause,
				$.SetClause,
				$.MergeClause,
				$.PatchClause,
				$.ReplaceClause,
				$.UnsetClause,
			),

		ContentClause: ($) => seq(alias($._kw_content, $.Keyword), $._value),
		SetClause: ($) =>
			seq(alias($._kw_set, $.Keyword), csep($.FieldAssignment)),
		MergeClause: ($) => seq(alias($._kw_merge, $.Keyword), $._value),
		PatchClause: ($) => seq(alias($._kw_patch, $.Keyword), $.Array),
		ReplaceClause: ($) => seq(alias($._kw_replace, $.Keyword), $.Object),
		// UNSET removes fields by name (`UNSET a, b`), so it takes a field
		// list — the same shape as OMIT — rather than assignments.
		UnsetClause: ($) =>
			seq(alias($._kw_unset, $.Keyword), csep($._inclusivePredicate)),
		OmitClause: ($) =>
			seq(alias($._kw_omit, $.Keyword), csep($._inclusivePredicate)),

		WhereClause: ($) =>
			seq(alias($._kw_where, $.Keyword), optional($._value)),

		WithClause: ($) =>
			seq(
				alias($._kw_with, $.Keyword),
				choice(
					alias($._kw_noindex, $.Keyword),
					seq(alias($._kw_index, $.Keyword), csep($.Ident)),
				),
			),

		SplitClause: ($) =>
			seq(
				alias($._kw_split, $.Keyword),
				optional(alias($._kw_on, $.Keyword)),
				$.Idiom,
			),

		GroupClause: ($) =>
			seq(
				alias($._kw_group, $.Keyword),
				choice(
					seq(optional(alias($._kw_by, $.Keyword)), csep($.Idiom)),
					alias($._kw_all, $.Keyword),
				),
			),

		OrderClause: ($) =>
			seq(
				alias($._kw_order, $.Keyword),
				optional(alias($._kw_by, $.Keyword)),
				choice(csep($.Order), $.FunctionCall),
			),
		Order: ($) =>
			seq(
				$.Idiom,
				optional(alias($._kw_collate, $.Keyword)),
				optional(alias($._kw_numeric, $.Keyword)),
				optional(
					choice(
						alias($._kw_asc, $.Keyword),
						alias($._kw_desc, $.Keyword),
					),
				),
			),

		LimitStartComboClause: ($) =>
			prec.right(
				choice(
					seq($.StartClause, optional($.LimitClause)),
					seq($.LimitClause, optional($.StartClause)),
				),
			),
		StartClause: ($) =>
			seq(
				alias($._kw_start, $.Keyword),
				optional(alias($._kw_at, $.Keyword)),
				choice($.Number, $.VariableName),
			),
		LimitClause: ($) =>
			seq(
				alias($._kw_limit, $.Keyword),
				optional(alias($._kw_by, $.Keyword)),
				choice($.Number, $.VariableName),
			),

		// FETCH takes a filtered idiom (`FETCH a[WHERE …]`) where SPLIT, GROUP
		// and ORDER do not — 3.2.3 rejects the filter in all three — so the
		// looser shape lives here and not in `Idiom` itself. The root may be a
		// keyword: `FETCH RETURN` fetches a field named `RETURN`, and arrives
		// as a `Keyword` root (see `_fetchKeywordRoot`), which lowering reads
		// as the field name.
		FetchClause: ($) =>
			seq(
				alias($._kw_fetch, $.Keyword),
				csep(alias($._fetchIdiom, $.Idiom)),
			),
		_fetchIdiom: ($) =>
			seq(
				choice($.Ident, $._fetchKeywordRoot),
				repeat(
					choice(
						seq('.', choice($.Ident, alias('*', $.Any))),
						seq('[', alias('*', $.Any), ']'),
						alias('...', $.Flatten),
						alias($._idiomFilter, $.Filter),
					),
				),
			),
		// Each keyword token aliased to `Keyword`, the way every other use of
		// it is. Neither `alias($._any_kw, $.Ident)` nor the `Keyword` rule
		// itself will do here: both reach the tokens unaliased (or aliased a
		// second way), which costs every keyword its simple alias and turns
		// it into a hidden token — one that is free to skip inside an ERROR
		// node, so MISSING recovery loses to ERROR recovery wherever a
		// keyword borders the gap.
		_fetchKeywordRoot: ($) =>
			choice(
				...KEYWORDS.map((name) => alias($[`_kw_${name}`], $.Keyword)),
			),
		// `[WHERE …]` / `[? …]`, the subset of `_pathFilter` that an idiom
		// position accepts. Kept separate from `_pathFilter` so that `[*]`
		// stays the idiom rules' own alternative rather than an ambiguity.
		_idiomFilter: ($) =>
			seq(
				'[',
				choice(
					$.WhereClause,
					alias($._questionWhere, $.WhereClause),
				),
				']',
			),
		TimeoutClause: ($) => seq(alias($._kw_timeout, $.Keyword), $.Duration),
		ParallelClause: ($) => alias($._kw_parallel, $.Keyword),
		TempfilesClause: ($) => alias($._kw_tempfiles, $.Keyword),
		ExplainClause: ($) =>
			seq(
				alias($._kw_explain, $.Keyword),
				optional(alias($._kw_full, $.Literal)),
			),
		VersionClause: ($) => seq(alias($._kw_version, $.Keyword), $.String),

		ReturnClause: ($) =>
			seq(
				alias($._kw_return, $.Keyword),
				choice(
					alias($._kw_before, $.Literal),
					alias($._kw_after, $.Literal),
					alias($._kw_diff, $.Literal),
					$.Fields,
				),
			),

		// ----------------------------------------------------------------
		// Other clauses
		// ----------------------------------------------------------------

		IfNotExistsClause: ($) =>
			seq(
				alias($._kw_if, $.Keyword),
				alias($._kw_not, $.Keyword),
				alias($._kw_exists, $.Keyword),
			),
		IfExistsClause: ($) =>
			seq(alias($._kw_if, $.Keyword), alias($._kw_exists, $.Keyword)),
		OverwriteClause: ($) => alias($._kw_overwrite, $.Keyword),

		OnTableClause: ($) =>
			seq(
				alias($._kw_on, $.Keyword),
				optional(alias($._kw_table, $.Keyword)),
				$._value,
			),

		_nsKeyword: ($) =>
			choice(
				alias($._kw_ns, $.Keyword),
				alias($._kw_namespace, $.Keyword),
			),
		_dbKeyword: ($) =>
			choice(
				alias($._kw_db, $.Keyword),
				alias($._kw_database, $.Keyword),
			),

		OnRootNsDbClause: ($) =>
			seq(
				alias($._kw_on, $.Keyword),
				choice(
					alias($._kw_root, $.Keyword),
					$._nsKeyword,
					$._dbKeyword,
				),
			),

		// TYPE JWT|RECORD|BEARER. Everything the engine parses as part of the
		// access type stays nested here: it rejects SIGNUP/SIGNIN once a WITH
		// clause has been seen, so this is genuinely ordered even though the
		// clauses around it (AUTHENTICATE/DURATION/COMMENT) are not.
		AccessTypeClause: ($) =>
			seq(
				alias($._kw_type, $.Keyword),
				choice(
					seq(alias($._kw_jwt, $.Keyword), $.JwtClause),
					seq(
						alias($._kw_record, $.Keyword),
						repeat(choice($.SignupClause, $.SigninClause)),
						optional($.RefreshClause),
						optional($.WithJwtClause),
						optional($.RefreshClause),
					),
					seq(
						alias($._kw_bearer, $.Keyword),
						alias($._kw_for, $.Keyword),
						choice(
							alias($._kw_user, $.Keyword),
							alias($._kw_record, $.Keyword),
						),
						optional($.WithJwtClause),
					),
				),
			),

		WithJwtClause: ($) =>
			seq(
				alias($._kw_with, $.Keyword),
				alias($._kw_jwt, $.Keyword),
				$.JwtClause,
			),
		RefreshClause: ($) =>
			seq(
				alias($._kw_with, $.Keyword),
				alias($._kw_refresh, $.Keyword),
			),

		JwtClause: ($) =>
			seq(
				choice(
					seq(
						alias($._kw_algorithm, $.Keyword),
						$.Ident,
						alias($._kw_key, $.Keyword),
						$._accessKeyValue,
					),
					seq(alias($._kw_url, $.Keyword), $._accessKeyValue),
				),
				optional($.IssuerClause),
			),

		// WITH ISSUER takes an algorithm, a key, both, or neither.
		IssuerClause: ($) =>
			seq(
				alias($._kw_with, $.Keyword),
				alias($._kw_issuer, $.Keyword),
				optional(seq(alias($._kw_algorithm, $.Keyword), $.Ident)),
				optional(
					seq(alias($._kw_key, $.Keyword), $._accessKeyValue),
				),
			),

		_accessKeyValue: ($) => choice($.String, $.VariableName),

		SignupClause: ($) => seq(alias($._kw_signup, $.Keyword), $._value),
		SigninClause: ($) => seq(alias($._kw_signin, $.Keyword), $._value),
		AuthenticateClause: ($) =>
			seq(alias($._kw_authenticate, $.Keyword), $._value),
		SessionClause: ($) => seq(alias($._kw_session, $.Keyword), $.Duration),

		// DURATION FOR <TOKEN|SESSION|GRANT> <duration|NONE>, ... — the engine
		// requires the FOR target on every entry and accepts NONE in place of
		// a duration to mean "never expires".
		DurationClause: ($) =>
			seq(alias($._kw_duration, $.Keyword), csep($.DurationValue)),
		DurationValue: ($) =>
			seq(
				alias($._kw_for, $.Keyword),
				choice(
					alias($._kw_token, $.Keyword),
					alias($._kw_session, $.Keyword),
					alias($._kw_grant, $.Keyword),
				),
				choice($.Duration, alias($._kw_none, $.None)),
			),

		TokenTypeClause: ($) => seq(alias($._kw_type, $.Keyword), $.TokenType),

		FieldsColumnsClause: ($) =>
			seq(
				choice(
					alias($._kw_fields, $.Keyword),
					alias($._kw_columns, $.Keyword),
				),
				csep($.Idiom),
			),

		IndexClause: ($) =>
			choice(
				$.UniqueClause,
				$.SearchAnalyzerClause,
				$.FulltextClause,
				$.MtreeClause,
				$.HnswClause,
				$.DiskannClause,
			),
		UniqueClause: ($) => alias($._kw_unique, $.Keyword),

		FulltextClause: ($) =>
			seq(
				alias($._kw_fulltext, $.Keyword),
				repeat(
					choice(
						seq(alias($._kw_analyzer, $.Keyword), $.Ident),
						$.Bm25Clause,
						alias($._kw_highlights, $.Keyword),
					),
				),
			),

		SearchAnalyzerClause: ($) =>
			seq(
				alias($._kw_search, $.Keyword),
				alias($._kw_analyzer, $.Keyword),
				$.Ident,
				repeat(
					choice(
						$.Bm25Clause,
						$.DocIdsOrderClause,
						$.DocLenghtsOrderClause,
						$.PostingsOrderClause,
						$.TermsOrderClause,
						$.DocIdsCacheClause,
						$.DocLenghtsCacheClause,
						$.PostingsCacheClause,
						$.TermsCacheClause,
						alias($._kw_highlights, $.Keyword),
					),
				),
			),

		Bm25Clause: ($) =>
			seq(
				alias($._kw_bm25, $.Keyword),
				optional(seq('(', $.Number, ',', $.Number, ')')),
			),
		DocIdsCacheClause: ($) =>
			seq(alias($._kw_doc_ids_cache, $.Keyword), $.Number),
		DocIdsOrderClause: ($) =>
			seq(alias($._kw_doc_ids_order, $.Keyword), $.Number),
		DocLenghtsCacheClause: ($) =>
			seq(alias($._kw_doc_lengths_cache, $.Keyword), $.Number),
		DocLenghtsOrderClause: ($) =>
			seq(alias($._kw_doc_lengths_order, $.Keyword), $.Number),
		PostingsCacheClause: ($) =>
			seq(alias($._kw_postings_cache, $.Keyword), $.Number),
		PostingsOrderClause: ($) =>
			seq(alias($._kw_postings_order, $.Keyword), $.Number),
		TermsCacheClause: ($) =>
			seq(alias($._kw_terms_cache, $.Keyword), $.Number),
		TermsOrderClause: ($) =>
			seq(alias($._kw_terms_order, $.Keyword), $.Number),

		MtreeClause: ($) =>
			seq(
				alias($._kw_mtree, $.Keyword),
				$.IndexDimensionClause,
				repeat(
					choice(
						$.MtreeDistClause,
						$.IndexTypeClause,
						$.IndexCapacityClause,
						$.DocIdsOrderClause,
						$.DocIdsCacheClause,
						$.MtreeCacheClause,
					),
				),
			),
		MtreeCacheClause: ($) =>
			seq(alias($._kw_mtree_cache, $.Keyword), $.Number),
		MtreeDistClause: ($) => seq(alias($._kw_dist, $.Keyword), $.Distance),

		HnswClause: ($) =>
			seq(
				alias($._kw_hnsw, $.Keyword),
				$.IndexDimensionClause,
				repeat(
					choice(
						$.HnswDistClause,
						$.IndexTypeClause,
						$.IndexCapacityClause,
						$.IndexLmClause,
						$.IndexM0Clause,
						$.IndexMClause,
						$.IndexEfcClause,
						$.IndexExtendCandidatesClause,
						$.IndexKeepPrunedConnectionsClause,
						$.IndexHashedVectorClause,
					),
				),
			),
		HnswDistClause: ($) =>
			seq(
				alias($._kw_dist, $.Keyword),
				choice(
					$.Distance,
					seq(alias($._kw_minkowski, $.Distance), $.Number),
				),
			),

		// DISKANN, the v3 replacement for MTREE. Its DEGREE/L_BUILD/ALPHA are
		// its own; DIMENSION is the one required part.
		DiskannClause: ($) =>
			seq(
				alias($._kw_diskann, $.Keyword),
				$.IndexDimensionClause,
				repeat(
					choice(
						$.HnswDistClause,
						$.IndexTypeClause,
						$.IndexDegreeClause,
						$.IndexLBuildClause,
						$.IndexAlphaClause,
						$.IndexHashedVectorClause,
					),
				),
			),
		IndexDegreeClause: ($) =>
			seq(alias($._kw_degree, $.Keyword), $.Number),
		IndexLBuildClause: ($) =>
			seq(alias($._kw_l_build, $.Keyword), $.Number),
		IndexAlphaClause: ($) =>
			seq(alias($._kw_alpha, $.Keyword), $.Number),
		IndexHashedVectorClause: ($) =>
			alias($._kw_hashed_vector, $.Keyword),

		IndexDimensionClause: ($) =>
			seq(alias($._kw_dimension, $.Keyword), $.Number),
		IndexCapacityClause: ($) =>
			seq(alias($._kw_capacity, $.Keyword), $.Number),
		IndexLmClause: ($) => seq(alias($._kw_lm, $.Keyword), $.Number),
		IndexM0Clause: ($) => seq(alias($._kw_m0, $.Keyword), $.Number),
		IndexMClause: ($) => seq(alias($._kw_m, $.Keyword), $.Number),
		IndexEfcClause: ($) => seq(alias($._kw_efc, $.Keyword), $.Number),
		IndexExtendCandidatesClause: ($) =>
			alias($._kw_extend_candidates, $.Keyword),
		IndexKeepPrunedConnectionsClause: ($) =>
			alias($._kw_keep_pruned_connections, $.Keyword),

		// Define table
		TableTypeClause: ($) =>
			seq(
				alias($._kw_type, $.Keyword),
				choice(
					alias($._kw_any, $.Keyword),
					alias($._kw_normal, $.Keyword),
					seq(
						alias($._kw_relation, $.Keyword),
						optional(
							seq(
								choice(
									alias($._kw_in, $.Keyword),
									alias($._kw_from, $.Keyword),
								),
								piped($.Ident),
							),
						),
						optional(
							seq(
								choice(
									alias($._kw_out, $.Keyword),
									alias($._kw_to, $.Keyword),
								),
								piped($.Ident),
							),
						),
						optional($.EnforcedClause),
					),
				),
			),
		EnforcedClause: ($) => alias($._kw_enforced, $.Keyword),

		TableViewClause: ($) =>
			seq(
				alias($._kw_as, $.Keyword),
				alias($._kw_select, $.Keyword),
				csep($._inclusivePredicate),
				alias($._kw_from, $.Keyword),
				csep($._value),
				optional($.WhereClause),
				optional($.GroupClause),
			),

		ChangefeedClause: ($) =>
			seq(
				alias($._kw_changefeed, $.Keyword),
				$.Duration,
				optional(
					seq(
						alias($._kw_include, $.Keyword),
						alias($._kw_original, $.Keyword),
					),
				),
			),

		WhenClause: ($) => seq(alias($._kw_when, $.Keyword), $._value),
		// THEN takes a comma-separated list of values, and a value here may be
		// a bare statement (`THEN RETURN 'foo'`, `THEN CREATE x`).
		ThenClause: ($) =>
			seq(
				alias($._kw_then, $.Keyword),
				// A value list, or `RETURN <value>`. The body cannot be an
				// arbitrary statement: DEFINE and ALTER own a COMMENT of their
				// own, so a trailing COMMENT would belong either to them or to
				// the event, and admitting them costs 14 GLR conflicts and
				// doubles the parser table. Write those bodies as a block.
				// Each body is a named rule under the alias: aliasing a bare
				// `seq` renames its members instead of wrapping them.
				choice(
					csep($._value),
					alias($._thenReturn, $.ReturnStatement),
					alias($._thenThrow, $.ThrowStatement),
				),
			),
		_thenReturn: ($) => seq(alias($._kw_return, $.Keyword), $._value),
		_thenThrow: ($) => seq(alias($._kw_throw, $.Keyword), $._value),
		// RETRY and MAXDEPTH only exist behind ASYNC — the engine says so by
		// name — but they may follow it in either order.
		AsyncClause: ($) =>
			seq(
				alias($._kw_async, $.Keyword),
				repeat(choice($.EventRetryClause, $.EventMaxDepthClause)),
			),
		EventRetryClause: ($) => seq(alias($._kw_retry, $.Keyword), $.Number),
		EventMaxDepthClause: ($) =>
			seq(alias($._kw_maxdepth, $.Keyword), $.Number),

		TokenizersClause: ($) =>
			seq(alias($._kw_tokenizers, $.Keyword), csep($.AnalyzerTokenizer)),
		FiltersClause: ($) =>
			seq(alias($._kw_filters, $.Keyword), csep($.AnalyzerFilters)),
		FunctionClause: ($) =>
			seq(alias($._kw_function, $.Keyword), $.FunctionName),

		TypeClause: ($) =>
			prec.right(
				choice(
					seq(
						alias($._kw_flexible, $.Keyword),
						alias($._kw_type, $.Keyword),
						$._type,
					),
					seq(
						alias($._kw_type, $.Keyword),
						$._type,
						alias($._kw_flexible, $.Keyword),
					),
					seq(alias($._kw_type, $.Keyword), $._type),
				),
			),

		DefaultClause: ($) =>
			seq(
				alias($._kw_default, $.Keyword),
				optional($.DefaultAlways),
				$._value,
			),
		DefaultAlways: ($) => alias($._kw_always, $.Keyword),

		ReadonlyClause: ($) => alias($._kw_readonly, $.Keyword),
		ValueClause: ($) =>
			seq(
				alias($._kw_value, $.Keyword),
				$._value,
			),
		AssertClause: ($) =>
			seq(
				alias($._kw_assert, $.Keyword),
				$._value,
			),
		ComputedClause: ($) =>
			seq(
				alias($._kw_computed, $.Keyword),
				$._value,
			),

		ReferenceClause: ($) =>
			seq(
				alias($._kw_reference, $.Keyword),
				optional(
					seq(
						alias($._kw_on, $.Keyword),
						alias($._kw_delete, $.Keyword),
						choice(
							alias($._kw_reject, $.Keyword),
							alias($._kw_cascade, $.Keyword),
							alias($._kw_ignore, $.Keyword),
							alias($._kw_unset, $.Keyword),
							seq(alias($._kw_then, $.Keyword), $.Block),
						),
					),
				),
			),

		PermissionGroup: ($) =>
			seq(
				alias($._kw_for, $.Keyword),
				csep(
					choice(
						alias($._kw_select, $.Keyword),
						alias($._kw_create, $.Keyword),
						alias($._kw_update, $.Keyword),
						alias($._kw_delete, $.Keyword),
					),
				),
				choice(
					$.WhereClause,
					alias($._kw_none, $.None),
					alias($._kw_full, $.Literal),
				),
			),

		PermissionsForClause: ($) =>
			seq(
				alias($._kw_permissions, $.Keyword),
				choice(
					alias($._kw_none, $.None),
					alias($._kw_full, $.Literal),
					// The engine takes the groups with or without commas
					// between them.
					seq(
						$.PermissionGroup,
						repeat(seq(optional(','), $.PermissionGroup)),
					),
				),
			),

		PermissionsBasicClause: ($) =>
			seq(
				alias($._kw_permissions, $.Keyword),
				choice(
					alias($._kw_none, $.None),
					alias($._kw_full, $.Literal),
					$.WhereClause,
				),
			),

		MiddlewareClause: ($) =>
			seq(alias($._kw_middleware, $.Keyword), csep($.FunctionCall)),
		CommentClause: ($) => seq(alias($._kw_comment, $.Keyword), $.String),
		BackendClause: ($) => seq(alias($._kw_backend, $.Keyword), $._value),

		AnalyzerFilters: ($) =>
			seq(
				alias($._analyzerFilterKw, $.Filter),
				optional(
					seq(
						'(',
						choice(seq($.Number, ',', $.Number), $.Ident),
						')',
					),
				),
			),

		// ================================================================
		// Values
		// ================================================================

		// IF is an expression in SurrealQL, not only a statement: it is legal
		// unparenthesised in a projection, a WHERE, an array element, an
		// object value, a SET right-hand side. It sits here rather than in
		// `_baseValue` so it does not also become a path or lookup base,
		// which the engine does not accept.
		_value: ($) =>
			choice(
				$.Path,
				$.BinaryExpression,
				$.Range,
				$.PrefixExpression,
				$.TypeCast,
				$._baseValue,
				$.IfElseStatement,
			),

		PrefixExpression: ($) =>
			prec(
				'prefix',
				seq(
					choice(
						alias('!', $.Operator),
						alias('-', $.Operator),
						alias('+', $.Operator),
						alias($._kw_not, $.Operator),
					),
					$._prefixOperand,
				),
			),
		_prefixOperand: ($) =>
			choice($.PrefixExpression, $.Path, $.TypeCast, $._baseValue),

		_baseValue: ($) =>
			choice(
				$._computedValue,
				$.FormatString,
				$.Regex,
				$.VariableName,
				$.FunctionJs,
				$.FunctionCall,
				$.SubQuery,
				$.Block,
				$.Closure,
				$.Ident,
				// Non-reserved clause keywords are valid identifiers where a value
				// (idiom) is expected — e.g. `WHERE order = $x`, `WHERE key = $y`.
				alias($._nonReservedIdent, $.Ident),
			),

		_nonReservedIdent: ($) =>
			choice(
				$._kw_order,
				$._kw_start,
				$._kw_limit,
				$._kw_group,
				$._kw_key,
			),

		_computedValue: ($) =>
			choice(
				$.String,
				$.Number,
				alias($._kw_true, $.Bool),
				alias($._kw_false, $.Bool),
				alias($._kw_null, $.None),
				alias($._kw_none, $.None),
				$.Array,
				$.Set,
				$.RecordId,
				$.Object,
				$.Duration,
				$.Point,
				$.Constant,
				// `|table:10|` and `|table:1..10|` generate records anywhere a
				// value is wanted, not only as a CREATE target.
				$.RangeRecordId,
			),

		// One token, at a higher lexical precedence than `FunctionName`, so
		// `math::PI` lexes as the constant while `math::pilot(…)` still lexes
		// as a function name (longest match settles that first).
		Constant: ($) =>
			token(
				prec(
					4,
					new RegExp(
						`(?:${kw('math').source}::(?:${kwAlt(MATH_CONSTANTS)})` +
							`|${kw('time').source}::${kw('EPOCH').source}` +
							`|${kw('duration').source}::${kw('MAX').source})`,
					),
				),
			),

		// Paths
		Path: ($) =>
			choice(
				seq($._baseValue, repeat1($._pathElement)),
				seq(
					$.At,
					choice(
						seq($._dotPart, repeat($._pathElement)),
						repeat1($._pathElement),
					),
				),
				seq($.Lookup, repeat($._pathElement)),
			),
		_pathElement: ($) =>
			choice(
				$.Lookup,
				$.Subscript,
				alias($._pathFilter, $.Filter),
				// `a?.b` optional chaining. Also reachable as a `_dotPart` after
				// `@`; either reading yields the same `Optional` node.
				prec(1, $.Optional),
				// `...` flattens the array the path has reached.
				alias('...', $.Flatten),
			),
		Subscript: ($) => seq('.', $._dotPart),
		_dotPart: ($) =>
			choice(
				$.At,
				$.Ident,
				$.IdiomFunction,
				alias('*', $.Any),
				$.Optional,
				$.Destructure,
				$.Recurse,
			),

		_pathFilter: ($) =>
			seq(
				'[',
				choice(
					$.WhereClause,
					// `[? value]` shorthand — wrap in WhereClause to match lezer's
					// inline `WhereClause { "?" value }` rule.
					alias($._questionWhere, $.WhereClause),
					// `[*]` selects every element, `[$]` the last one.
					alias('*', $.Any),
					alias('$', $.Last),
					$._expression,
				),
				']',
			),
		_questionWhere: ($) => seq('?', $._value),

		Lookup: ($) =>
			seq(
				choice($.LookupRight, $.LookupLeft, $.LookupBoth),
				choice($.Ident, $.Any, $.LookupSelection),
			),

		LookupSelection: ($) =>
			seq(
				'(',
				optional($.GraphFieldSelection),
				csep($.GraphPredicate),
				repeat(
					choice(
						$.WhereClause,
						alias($.SplitClause, $.GraphSplitClause),
						alias($.GroupClause, $.GraphGroupClause),
						alias($.OrderClause, $.GraphOrderClause),
						alias(
							$.LimitStartComboClause,
							$.GraphLimitStartComboClause,
						),
						seq(alias($._kw_as, $.Keyword), $.Ident),
					),
				),
				')',
			),
		GraphFieldSelection: ($) =>
			seq(
				alias($._kw_select, $.Keyword),
				$.Fields,
				alias($._kw_from, $.Keyword),
			),
		// `FIELD <name>` names the edge field to traverse, and binds to the
		// predicate it follows rather than to the selection as a whole:
		// `(message FIELD author, b FIELD c)` gives each table its own field.
		// The name is an Ident and only an Ident — no path, no param.
		GraphPredicate: ($) =>
			choice(
				seq($._value, optional($.GraphFieldClause)),
				$.Any,
			),
		GraphFieldClause: ($) =>
			seq(alias($._kw_field, $.Keyword), $.Ident),

		// The selection list is OPTIONAL: `id.{}` is valid SurrealQL and
		// evaluates to the empty object (3.0.5: `SELECT VALUE id.{} FROM ONLY
		// user:ada` -> `{}`). Requiring at least one entry made every such
		// expression a parse error, which is fatal to the whole source.
		Destructure: ($) =>
			seq(
				$.BraceOpen,
				optional(
					csep(
						choice(
							seq(
								$.Ident,
								$.Colon,
								choice(
									seq($.Lookup, repeat($._pathElement)),
									$._value,
								),
							),
							seq(choice($.Ident, $.Lookup), repeat($._pathElement)),
						),
					),
				),
				$.BraceClose,
			),

		IdiomFunction: ($) =>
			seq(alias($._rawident, $.FunctionName), $.ArgumentList),

		Recurse: ($) =>
			seq(
				$.BraceOpen,
				$.RecurseRange,
				optional($.RecurseOptions),
				$.BraceClose,
				optional(seq('(', repeat1($._pathElement), ')')),
			),
		RecurseRange: ($) =>
			prec.right(
				choice(
					seq($.Int, $.RangeOp, $.Int),
					seq($.Int, $.RangeOp),
					$.RangeOp,
					seq($.RangeOp, $.Int),
					$.Int,
				),
			),
		RecurseOptions: ($) =>
			repeat1(
				seq(
					'+',
					alias($._rawident, $.FunctionName),
					optional(seq('=', $._baseValue)),
				),
			),

		// Idiom
		Idiom: ($) =>
			seq(
				$.Ident,
				repeat(
					choice(
						seq('.', choice($.Ident, alias('*', $.Any))),
						seq('[', alias('*', $.Any), ']'),
						// `...` flattens the array the path has reached.
						alias('...', $.Flatten),
					),
				),
			),

		// Binary expression
		//
		// Operators are grouped into precedence tiers whose order mirrors
		// surrealdb-core's `BindingPower` enum, tightest to loosest:
		// power > multiplicative > additive > relation > equality >
		// conjunction (AND) > disjunction (OR) > nullish (?? / ?:).
		// Each tier still surfaces a single `Operator` node, so the CST node
		// types are unchanged — only the nesting is corrected. This is what
		// makes `a > 1 AND b > 2` parse as `(a > 1) AND (b > 2)` rather than
		// the previous flat-left `((a > 1) AND b) > 2`, and it also gives the
		// engine-faithful `a = (b < c)` and `a ?: (b OR c)` nestings.
		BinaryExpression: ($) => {
			const tier = (level, ops) =>
				prec.left(
					level,
					seq($._value, alias(ops, $.Operator), $._value),
				);
			return choice(
				tier('binary_nullish', $._binop_nullish),
				tier('binary_disjunction', $._binop_disjunction),
				tier('binary_conjunction', $._binop_conjunction),
				tier('binary_equality', $._binop_equality),
				tier('binary_relation', $._binop_relation),
				tier('binary_additive', $._binop_additive),
				tier('binary_multiplicative', $._binop_multiplicative),
				tier('binary_power', $._binop_power),
			);
		},

		// Nullish coalescing / ternary — looser than OR (BindingPower::Nullish).
		_binop_nullish: ($) => choice('??', '?:'),
		_binop_disjunction: ($) => choice($._kw_or, '||'),
		_binop_conjunction: ($) => choice($._kw_and, '&&'),
		// Equality family (BindingPower::Equality): =, ==, !=, ?=, *=, IS,
		// IS NOT, the fuzzy-match operators, and the full-text @@ / @ref@.
		_binop_equality: ($) =>
			choice(
				'=',
				'==',
				'!=',
				'?=',
				'*=',
				'~',
				'!~',
				'*~',
				'?~',
				$._kw_is,
				// `a IS NOT b` is one operator, never `a IS (NOT b)`.
				prec(1, seq($._kw_is, $._kw_not)),
				'@@',
				seq('@', $.Number, '@'),
			),
		// Relational family (BindingPower::Relation): ordering, membership,
		// containment, geo, and the KNN operator.
		_binop_relation: ($) =>
			choice(
				'<',
				'<=',
				'>',
				'>=',
				alias($._kw_in, $.Keyword),
				seq($._kw_not, alias($._kw_in, $.Keyword)),
				$._kw_contains,
				$._kw_containsnot,
				$._kw_containsall,
				$._kw_containsany,
				$._kw_containsnone,
				$._kw_inside,
				$._kw_notinside,
				$._kw_allinside,
				$._kw_anyinside,
				$._kw_noneinside,
				$._kw_outside,
				$._kw_intersects,
				seq(
					'<|',
					$.Number,
					optional(
						seq(
							',',
							choice(
								$.Number,
								$.Distance,
								seq(
									alias($._kw_minkowski, $.Distance),
									$.Number,
								),
							),
						),
					),
					'|>',
				),
				...['∋', '∌', '⊇', '⊃', '⊅', '∈', '∉', '⊆', '⊂', '⊄'],
			),
		_binop_additive: ($) => choice('+', '-', '+=', '-='),
		_binop_multiplicative: ($) => choice('*', '×', '/', '÷', '%'),
		_binop_power: ($) => '**',

		// Range
		Range: ($) =>
			prec.left(
				'range',
				choice(
					$.RangeOp,
					seq($._value, $.RangeOp),
					seq($.RangeOp, $._value),
					seq($._value, $.RangeOp, $._value),
				),
			),

		// Type cast. The operand is a whole value, and the 'cast' precedence
		// settles what that value reaches: a range, a path, a prefix operator
		// or another cast is inside the cast (`<array> 1..5`, `<string> $x.y`,
		// `<string> -$x`); a binary operator is outside it (`<string> 1 + 2`
		// is `(<string> 1) + 2`). Mirrors 3.2.3.
		TypeCast: ($) => prec('cast', seq('<', $._type, '>', $._value)),

		// Closure
		Closure: ($) =>
			prec(
				'closure',
				choice(
					seq(
						$.Pipe,
						optional(csep($.ParamDefinition)),
						$.Pipe,
						optional(seq($.LookupRight, $._type)),
						$.Block,
					),
					// Bare-expression body, e.g. `|$v, $i| $v` or `|$v| $v * 2`.
					// Any value is allowed, not just a BinaryExpression.
					seq(
						$.Pipe,
						optional(csep($.ParamDefinition)),
						$.Pipe,
						$._value,
					),
				),
			),

		ParamDefinition: ($) =>
			seq(
				$.VariableName,
				optional(seq($.Colon, alias($._safeType, $.Type))),
			),

		// Block / SubQuery
		Block: ($) => seq($.BraceOpen, optional($._expressions), $.BraceClose),

		SubQuery: ($) => seq('(', $._expression, ')'),

		// ----------------------------------------------------------------
		// Object/Array/Set/Point
		// ----------------------------------------------------------------

		Object: ($) =>
			seq(
				alias($._object_open, $.BraceOpen),
				optional($.ObjectContent),
				$.BraceClose,
			),
		ObjectContent: ($) => csepTrail($.ObjectProperty),
		ObjectProperty: ($) =>
			seq(
				$.ObjectKey,
				$.Colon,
				choice(
					alias($._objectSelectValue, $.SelectStatement),
					$._value,
				),
			),
		ObjectKey: ($) => choice(alias($._rawident, $.KeyName), $.String),

		_objectSelectValue: ($) =>
			seq(
				alias($._kw_select, $.Keyword),
				alias($._kw_value, $.Keyword),
				$.Predicate,
				alias($._kw_from, $.Keyword),
				optional(alias($._kw_only, $.Keyword)),
				$._value,
			),

		Array: ($) => seq('[', optional(csepTrail($._value)), ']'),

		Set: ($) =>
			seq(
				$.BraceOpen,
				choice(
					',',
					seq($._value, ','),
					seq($._value, ',', $._value, repeat(seq(',', $._value))),
				),
				$.BraceClose,
			),

		Point: ($) => seq('(', $.Number, ',', $.Number, ')'),

		// ----------------------------------------------------------------
		// Record ID
		// ----------------------------------------------------------------

		RecordId: ($) =>
			seq(
				alias($._idName, $.RecordTbIdent),
				$.Colon,
				choice($._recordIdValue, $.RecordIdRange),
			),
		RangeRecordId: ($) => seq($.Pipe, $.RecordId, $.Pipe),
		_idName: ($) => choice($._rawident, $._tickIdent, $._bracketIdent),
		RecordIdIdent: ($) =>
			choice($._numberident, $._tickIdent, $._bracketIdent),
		_recordIdValue: ($) =>
			choice($.RecordIdIdent, $.Array, $.Object, $.RecordIdString),
		// Lezer emits RecordIdString(String); we wrap the prefixed-string token
		// in an aliased String node to match the same structure.
		RecordIdString: ($) => alias($._prefixedString, $.String),
		RecordIdRange: ($) =>
			prec.right(
				choice(
					$.RangeOp,
					seq($._recordIdValue, $.RangeOp, $._recordIdValue),
					seq($._recordIdValue, $.RangeOp),
					seq($.RangeOp, $._recordIdValue),
				),
			),

		// ----------------------------------------------------------------
		// Function call (regular, custom, idiom-relative)
		// ----------------------------------------------------------------

		FunctionCall: ($) =>
			choice(
				prec.dynamic(
					1,
					seq(
						choice(
							$.FunctionName,
							alias($._kw_rand, $.FunctionName),
							alias($._kw_count, $.FunctionName),
							alias($._kw_not, $.FunctionName),
							alias($._kw_sleep, $.FunctionName),
						),
						optional($.Version),
						$.ArgumentList,
					),
				),
				seq($.RecordId, $.ArgumentList),
				seq($.VariableName, $.ArgumentList),
				// `(|$x| $x + 1)(41)` — a parenthesized value called in place
				// (3.2.3 evaluates it to 42; `(1 + 2)(3)` parses and fails at
				// run time as "'int' is not a function").
				seq($.SubQuery, $.ArgumentList),
			),
		// `not(x)` is the function, never `NOT (x)` the prefix operator over a
		// parenthesized value — the two mean the same thing, so the call form
		// wins statically.
		ArgumentList: ($) =>
			prec(
				1,
				seq(
					'(',
					optional(
						choice(
							csep($._value),
							$._subqueryStatement,
						),
					),
					')',
				),
			),
		Version: ($) => seq('<', $.VersionNumber, '>'),

		FunctionName: ($) =>
			choice(
				token(
					prec(
						3,
						seq(
							/[a-zA-Z_][a-zA-Z_0-9]*/,
							'::',
							/[a-zA-Z_][a-zA-Z_0-9]*/,
							repeat(seq('::', /[a-zA-Z_][a-zA-Z_0-9]*/)),
						),
					),
				),
				token(
					prec(
						3,
						seq('fn', repeat(seq('::', /[a-zA-Z_][a-zA-Z_0-9]*/))),
					),
				),
			),

		// ----------------------------------------------------------------
		// JS function
		// ----------------------------------------------------------------

		FunctionJs: ($) =>
			seq(
				alias($._kw_function, $.FunctionName),
				$.ArgumentList,
				$.JavaScriptBlock,
			),
		// The external scanner consumes the entire `{...}` block as one
		// token. We can't currently expose `BraceOpen`/`JavaScriptContent`/
		// `BraceClose` separately because the parser would invoke the scanner
		// in stray `{`-adjacent recovery states (e.g. after `[1f, 2f, …]`)
		// and silently eat the rest of the input. See the lezer-issues
		// catalog for the known divergence.
		JavaScriptBlock: ($) => $._js_function_body,

		// ----------------------------------------------------------------
		// Field assignment
		// ----------------------------------------------------------------

		// The target may reach into the record: `SET a.b += 1`, `SET a[WHERE …]
		// = 1`. A bare name still yields `Ident`, so only the nested form is
		// new to consumers.
		FieldAssignment: ($) =>
			seq(
				choice($.Ident, alias($._nestedAssignTarget, $.Idiom)),
				alias($._assignmentOp, $.Operator),
				$._value,
			),
		_nestedAssignTarget: ($) =>
			seq(
				$.Ident,
				repeat1(
					choice(
						seq('.', choice($.Ident, alias('*', $.Any))),
						seq('[', alias('*', $.Any), ']'),
						alias('...', $.Flatten),
						alias($._idiomFilter, $.Filter),
					),
				),
			),
		// `+?=` extends an array only where the value is missing. There is no
		// `-?=`: 3.2.3 rejects it.
		_assignmentOp: ($) => choice('=', '+=', '-=', '+?='),

		// ----------------------------------------------------------------
		// Fields & predicates
		// ----------------------------------------------------------------

		Fields: ($) =>
			choice(
				seq(alias($._kw_value, $.Keyword), $.Predicate),
				csep($._inclusivePredicate),
			),
		Predicate: ($) =>
			choice(
				$._value,
				seq($._value, alias($._kw_as, $.Keyword), $.Ident),
			),
		_inclusivePredicate: ($) => choice(alias('*', $.Any), $.Predicate),

		// ----------------------------------------------------------------
		// Types
		// ----------------------------------------------------------------

		_singleType: ($) =>
			choice(
				alias($._rawident, $.TypeName),
				$.ParameterizedType,
				$.LiteralType,
			),
		// `array<int, 3>` and `set<int, 5>` carry a size after the element
		// type; nothing else takes a second argument.
		ParameterizedType: ($) =>
			seq(
				$._singleType,
				'<',
				$._type,
				optional(seq(',', $.Number)),
				'>',
			),
		_type: ($) => choice($._singleType, $.UnionType),
		UnionType: ($) =>
			prec.right(
				'union',
				seq($._singleType, repeat1(seq($.Pipe, $._singleType))),
			),
		_safeType: ($) => choice($._singleType, seq('<', $._type, '>')),

		LiteralType: ($) =>
			choice($.String, $.Number, $.Duration, $.ArrayType, $.ObjectType),
		ArrayType: ($) => seq('[', csep($._type), ']'),
		ObjectType: ($) =>
			seq(
				alias($._object_open, $.BraceOpen),
				optional($.ObjectTypeContent),
				$.BraceClose,
			),
		ObjectTypeContent: ($) => csepTrail($.ObjectTypeProperty),
		ObjectTypeProperty: ($) => seq($.ObjectKey, $.Colon, $._type),

		// ================================================================
		// Lexical primitives
		// ================================================================

		Comment: ($) =>
			token(
				choice(
					seq('#', /[^\n]*/),
					seq('--', /[^\n]*/),
					seq('//', /[^\n]*/),
				),
			),

		BlockComment: ($) => token(seq('/*', /[^*]*\*+([^/*][^*]*\*+)*/, '/')),

		Number: ($) =>
			choice(
				prec.dynamic(1, seq(choice('-', '+'), $._unsignedNumber)),
				$._unsignedNumber,
			),
		_unsignedNumber: ($) => choice($.Decimal, $.Float, $.Int),

		Int: ($) => token(DIGITS),

		Float: ($) =>
			token(
				prec(
					1,
					choice(
						seq(DIGITS, 'f'),
						seq(
							DIGITS,
							choice(
								seq(
									'.',
									DIGITS,
									optional(/[eE][+-]?[0-9]+(?:_[0-9]+)*/),
								),
								/[eE][+-]?[0-9]+(?:_[0-9]+)*/,
							),
							optional('f'),
						),
						'Infinity',
						'NaN',
					),
				),
			),

		// Above `Float`'s precedence, because lexical precedence outranks
		// longest match: without it `9.7e-7dec` lexes as the float `9.7e-7`
		// followed by a stray `dec`.
		Decimal: ($) =>
			token(
				prec(
					2,
					seq(
						DIGITS,
						optional(seq('.', DIGITS)),
						optional(/[eE][+-]?[0-9]+(?:_[0-9]+)*/),
						'dec',
					),
				),
			),

		String: ($) =>
			choice($._stringLiteral, $._prefixedString, $._uuidString),
		// Lezer allows `\<newline>` and any other escape; we use [\s\S] to
		// include newlines after a backslash.
		_stringLiteral: ($) =>
			token(
				choice(
					seq("'", repeat(choice(/[^'\\]/, /\\[\s\S]/)), "'"),
					seq('"', repeat(choice(/[^"\\]/, /\\[\s\S]/)), '"'),
				),
			),
		_prefixedString: ($) =>
			token(
				prec(
					1,
					seq(
						/[rudbfs]/,
						choice(
							seq("'", repeat(choice(/[^'\\]/, /\\[\s\S]/)), "'"),
							seq('"', repeat(choice(/[^"\\]/, /\\[\s\S]/)), '"'),
						),
					),
				),
			),
		// The `u`-prefixed string is split out of `_prefixedString` (and lexes
		// above it) purely so KILL can demand one: a UUID literal is the only
		// literal KILL takes, and `KILL "…"` is a parse error in 3.2.3. It is
		// still aliased to `String` everywhere, so the tree shape is unchanged.
		_uuidString: ($) =>
			token(
				prec(
					2,
					seq(
						'u',
						choice(
							seq("'", repeat(choice(/[^'\\]/, /\\[\s\S]/)), "'"),
							seq('"', repeat(choice(/[^"\\]/, /\\[\s\S]/)), '"'),
						),
					),
				),
			),

		Regex: ($) =>
			token(
				prec(
					-1,
					seq(
						'/',
						repeat1(
							choice(
								/[^/\\\n\[]/,
								seq('\\', /[^\n]/),
								seq(
									'[',
									repeat(
										choice(/[^\n\\\]]/, seq('\\', /[^\n]/)),
									),
									']',
								),
							),
						),
						optional(seq('/', /[dgimsuvy]*/)),
					),
				),
			),

		VariableName: ($) =>
			token(
				seq(
					'$',
					choice(
						/[a-zA-Z_][a-zA-Z0-9_]*/,
						seq('`', /[^`]+/, '`'),
						seq('⟨', /[^⟩]+/, '⟩'),
					),
				),
			),

		Duration: ($) => repeat1($.DurationPart),

		DurationPart: ($) =>
			token(
				seq(
					DIGITS,
					/\s*/,
					choice(
						'ns',
						'us',
						'µs',
						'ms',
						's',
						'm',
						'h',
						'd',
						'w',
						'y',
					),
				),
			),

		// Format string with structured Interpolation nodes (mirrors lezer's
		// `FormatString { '$"' (content | Interpolation)* '"' | ... }`). The
		// content tokens use `prec(-1)` so they never outrank a real
		// expression-level token that could appear after error recovery.
		FormatString: ($) =>
			choice(
				seq(
					'$"',
					repeat(choice($._formatStringTextDouble, $.Interpolation)),
					'"',
				),
				seq(
					"$'",
					repeat(choice($._formatStringTextSingle, $.Interpolation)),
					"'",
				),
			),
		_formatStringTextDouble: ($) => token(prec(-1, /([^"\\{]|\\[\s\S])+/)),
		_formatStringTextSingle: ($) => token(prec(-1, /([^'\\{]|\\[\s\S])+/)),
		Interpolation: ($) => seq($.BraceOpen, $._expression, $.BraceClose),

		Ident: ($) => $._idName,

		_rawident: ($) => token(prec(-1, /[a-zA-Z_][a-zA-Z0-9_]*/)),
		_tickIdent: ($) => token(seq('`', /[^`]+/, '`')),
		_bracketIdent: ($) => token(seq('⟨', /[^⟩]+/, '⟩')),
		_numberident: ($) =>
			token(choice(/[a-zA-Z_][a-zA-Z0-9_]*/, /[0-9][a-zA-Z0-9_]*/)),

		VersionNumber: ($) =>
			token(
				seq(
					DIGITS,
					optional(seq('.', DIGITS, optional(seq('.', DIGITS)))),
				),
			),

		// ================================================================
		// Visible token-as-node rules
		// ================================================================

		Keyword: ($) => $._any_kw,
		// Operator. Mirrors lezer's tree shape:
		//   - In lezer the `in` keyword has `[@name=Keyword]` (visible) — so
		//     `Operator(Keyword)` for `IN`. `is`, `not`, and the
		//     `binaryOperatorKeyword` group (AND, OR, CONTAINS, …) are
		//     internal extend tokens without an `@name`, so the keyword text
		//     is consumed but not shown in the tree: `Operator` only.
		// The binary-operator alphabet, kept as a single symbol so the `!`
		// prefix and field assignments can still alias to `$.Operator`. Binary
		// expressions consume these via the precedence tiers above rather than
		// this rule directly.
		Operator: ($) =>
			choice(
				$._binop_nullish,
				$._binop_disjunction,
				$._binop_conjunction,
				$._binop_equality,
				$._binop_relation,
				$._binop_additive,
				$._binop_multiplicative,
				$._binop_power,
			),
		RangeOp: ($) => choice('..', '..=', '>..', '>..='),
		BraceOpen: ($) => '{',
		BraceClose: ($) => '}',
		Colon: ($) => ':',
		Pipe: ($) => '|',
		LookupRight: ($) => '->',
		LookupLeft: ($) => choice('<-', '<~'),
		LookupBoth: ($) => '<->',
		Any: ($) => choice('?', '*'),
		At: ($) => '@',
		Optional: ($) => '?',
		Bool: ($) => choice($._kw_true, $._kw_false),
		None: ($) => choice($._kw_null, $._kw_none),
		Literal: ($) =>
			choice($._kw_after, $._kw_before, $._kw_diff, $._kw_full),

		Distance: ($) =>
			choice(
				$._kw_chebyshev,
				$._kw_cosine,
				$._kw_cosine_normalized,
				$._kw_inner_product,
				$._kw_euclidean,
				$._kw_hamming,
				$._kw_jaccard,
				$._kw_manhattan,
				$._kw_minkowski,
				$._kw_pearson,
			),

		_analyzerFilterKw: ($) =>
			choice(
				$._kw_ascii,
				$._kw_edgengram,
				$._kw_ngram,
				$._kw_snowball,
				$._kw_uppercase,
				$._kw_lowercase,
			),

		AnalyzerTokenizer: ($) =>
			choice($._kw_blank, $._kw_camel, $._kw_class, $._kw_punct),

		TokenType: ($) =>
			choice(
				$._kw_jwks,
				$._kw_eddsa,
				$._kw_es256,
				$._kw_es384,
				$._kw_es512,
				$._kw_hs256,
				$._kw_hs384,
				$._kw_hs512,
				$._kw_ps256,
				$._kw_ps384,
				$._kw_ps512,
				$._kw_rs256,
				$._kw_rs384,
				$._kw_rs512,
			),

		HttpMethod: ($) =>
			choice(
				$._kw_get,
				$._kw_put,
				$._kw_post,
				$._kw_delete,
				$._kw_patch,
				$._kw_trace,
			),

		IndexTypeClause: ($) =>
			seq(
				optional(alias($._kw_type, $.Keyword)),
				choice(
					alias($._kw_f16, $.Keyword),
					alias($._kw_f32, $.Keyword),
					alias($._kw_f64, $.Keyword),
					alias($._kw_i8, $.Keyword),
					alias($._kw_i16, $.Keyword),
					alias($._kw_i32, $.Keyword),
					alias($._kw_i64, $.Keyword),
					alias($._kw_u8, $.Keyword),
				),
			),

		// ================================================================
		// Hidden token-source rules
		// ================================================================

		// ================================================================
		// Keyword tokens
		// ================================================================

		_kw_true: ($) => kw('true'),
		_kw_false: ($) => kw('false'),
		_kw_null: ($) => kw('null'),
		_kw_none: ($) => kw('none'),
		_kw_after: ($) => kw('after'),
		_kw_before: ($) => kw('before'),
		_kw_diff: ($) => kw('diff'),
		_kw_full: ($) => kw('full'),

		_kw_access: ($) => kw('access'),
		_kw_algorithm: ($) => kw('algorithm'),
		_kw_all: ($) => kw('all'),
		_kw_alter: ($) => kw('alter'),
		_kw_always: ($) => kw('always'),
		_kw_analyzer: ($) => kw('analyzer'),
		_kw_and: ($) => kw('and'),
		_kw_any: ($) => kw('any'),
		_kw_api: ($) => kw('api'),
		_kw_as: ($) => kw('as'),
		_kw_asc: ($) => kw('asc'),
		_kw_assert: ($) => kw('assert'),
		_kw_at: ($) => kw('at'),
		_kw_async: ($) => kw('async'),
		_kw_authenticate: ($) => kw('authenticate'),
		_kw_auto: ($) => kw('auto'),
		_kw_backend: ($) => kw('backend'),
		_kw_begin: ($) => kw('begin'),
		_kw_bm25: ($) => kw('bm25'),
		_kw_break: ($) => kw('break'),
		_kw_bucket: ($) => kw('bucket'),
		_kw_by: ($) => kw('by'),
		_kw_cancel: ($) => kw('cancel'),
		_kw_capacity: ($) => kw('capacity'),
		_kw_cascade: ($) => kw('cascade'),
		_kw_changefeed: ($) => kw('changefeed'),
		_kw_changes: ($) => kw('changes'),
		_kw_collate: ($) => kw('collate'),
		_kw_columns: ($) => kw('columns'),
		_kw_comment: ($) => kw('comment'),
		_kw_commit: ($) => kw('commit'),
		_kw_computed: ($) => kw('computed'),
		_kw_concurrently: ($) => kw('concurrently'),
		_kw_config: ($) => kw('config'),
		_kw_content: ($) => kw('content'),
		_kw_continue: ($) => kw('continue'),
		_kw_create: ($) => kw('create'),
		_kw_database: ($) => kw('database'),
		_kw_db: ($) => kw('db'),
		_kw_default: ($) => kw('default'),
		_kw_defer: ($) => kw('defer'),
		_kw_define: ($) => kw('define'),
		_kw_delete: ($) => kw('delete'),
		_kw_desc: ($) => kw('desc'),
		_kw_dimension: ($) => kw('dimension'),
		// v2 spelled it DIST, v3 spells it DISTANCE; both still parse.
		_kw_dist: ($) => choice(kw('dist'), kw('distance')),
		_kw_doc_ids_cache: ($) => kw('doc_ids_cache'),
		_kw_doc_ids_order: ($) => kw('doc_ids_order'),
		_kw_doc_lengths_cache: ($) => kw('doc_lengths_cache'),
		_kw_doc_lengths_order: ($) => kw('doc_lengths_order'),
		_kw_drop: ($) => kw('drop'),
		_kw_duplicate: ($) => kw('duplicate'),
		_kw_duration: ($) => kw('duration'),
		_kw_efc: ($) => kw('efc'),
		_kw_else: ($) => kw('else'),
		_kw_end: ($) => kw('end'),
		_kw_enforced: ($) => kw('enforced'),
		_kw_event: ($) => kw('event'),
		_kw_exclude: ($) => kw('exclude'),
		_kw_exists: ($) => kw('exists'),
		_kw_explain: ($) => kw('explain'),
		_kw_expunge: ($) => kw('expunge'),
		_kw_extend_candidates: ($) => kw('extend_candidates'),
		_kw_fetch: ($) => kw('fetch'),
		_kw_field: ($) => kw('field'),
		_kw_fields: ($) => kw('fields'),
		_kw_filters: ($) => kw('filters'),
		_kw_flexible: ($) => kw('flexible'),
		_kw_for: ($) => kw('for'),
		_kw_from: ($) => kw('from'),
		_kw_function: ($) => kw('function'),
		_kw_functions: ($) => kw('functions'),
		_kw_get: ($) => kw('get'),
		_kw_graphql: ($) => kw('graphql'),
		_kw_group: ($) => kw('group'),
		_kw_highlights: ($) => kw('highlights'),
		_kw_hnsw: ($) => kw('hnsw'),
		_kw_if: ($) => kw('if'),
		_kw_ignore: ($) => kw('ignore'),
		_kw_in: ($) => kw('in'),
		_kw_include: ($) => kw('include'),
		_kw_index: ($) => kw('index'),
		_kw_info: ($) => kw('info'),
		_kw_insert: ($) => kw('insert'),
		_kw_into: ($) => kw('into'),
		_kw_is: ($) => kw('is'),
		_kw_issuer: ($) => kw('issuer'),
		_kw_jwt: ($) => kw('jwt'),
		_kw_keep_pruned_connections: ($) => kw('keep_pruned_connections'),
		_kw_key: ($) => kw('key'),
		_kw_kill: ($) => kw('kill'),
		_kw_let: ($) => kw('let'),
		_kw_limit: ($) => kw('limit'),
		_kw_live: ($) => kw('live'),
		_kw_lm: ($) => kw('lm'),
		_kw_m: ($) => kw('m'),
		_kw_m0: ($) => kw('m0'),
		_kw_merge: ($) => kw('merge'),
		_kw_middleware: ($) => kw('middleware'),
		_kw_diskann: ($) => kw('diskann'),
		_kw_degree: ($) => kw('degree'),
		_kw_l_build: ($) => kw('l_build'),
		_kw_alpha: ($) => kw('alpha'),
		_kw_hashed_vector: ($) => kw('hashed_vector'),
		_kw_retry: ($) => kw('retry'),
		_kw_maxdepth: ($) => kw('maxdepth'),
		_kw_mtree: ($) => kw('mtree'),
		_kw_mtree_cache: ($) => kw('mtree_cache'),
		_kw_namespace: ($) => kw('namespace'),
		_kw_noindex: ($) => kw('noindex'),
		_kw_normal: ($) => kw('normal'),
		_kw_not: ($) => kw('not'),
		_kw_ns: ($) => kw('ns'),
		_kw_numeric: ($) => kw('numeric'),
		_kw_omit: ($) => kw('omit'),
		_kw_on: ($) => kw('on'),
		_kw_only: ($) => kw('only'),
		_kw_option: ($) => kw('option'),
		_kw_or: ($) => kw('or'),
		_kw_order: ($) => kw('order'),
		_kw_out: ($) => kw('out'),
		_kw_overwrite: ($) => kw('overwrite'),
		_kw_parallel: ($) => kw('parallel'),
		_kw_param: ($) => kw('param'),
		_kw_passhash: ($) => kw('passhash'),
		_kw_password: ($) => kw('password'),
		_kw_patch: ($) => kw('patch'),
		_kw_permissions: ($) => kw('permissions'),
		_kw_post: ($) => kw('post'),
		_kw_postings_cache: ($) => kw('postings_cache'),
		_kw_postings_order: ($) => kw('postings_order'),
		_kw_put: ($) => kw('put'),
		_kw_readonly: ($) => kw('readonly'),
		_kw_rebuild: ($) => kw('rebuild'),
		_kw_record: ($) => kw('record'),
		_kw_reference: ($) => kw('reference'),
		_kw_reject: ($) => kw('reject'),
		_kw_relate: ($) => kw('relate'),
		_kw_relation: ($) => kw('relation'),
		_kw_remove: ($) => kw('remove'),
		_kw_replace: ($) => kw('replace'),
		_kw_return: ($) => kw('return'),
		_kw_roles: ($) => kw('roles'),
		_kw_root: ($) => kw('root'),
		_kw_sc: ($) => kw('sc'),
		// The engine takes both spellings.
		_kw_schemafull: ($) => choice(kw('schemaful'), kw('schemafull')),
		_kw_schemaless: ($) => kw('schemaless'),
		_kw_scope: ($) => kw('scope'),
		_kw_search: ($) => kw('search'),
		_kw_select: ($) => kw('select'),
		_kw_session: ($) => kw('session'),
		_kw_set: ($) => kw('set'),
		_kw_show: ($) => kw('show'),
		_kw_signin: ($) => kw('signin'),
		_kw_signup: ($) => kw('signup'),
		_kw_since: ($) => kw('since'),
		_kw_sleep: ($) => kw('sleep'),
		_kw_split: ($) => kw('split'),
		_kw_start: ($) => kw('start'),
		_kw_strict: ($) => kw('strict'),
		_kw_structure: ($) => kw('structure'),
		_kw_table: ($) => kw('table'),
		_kw_tables: ($) => kw('tables'),
		_kw_tb: ($) => kw('tb'),
		_kw_tempfiles: ($) => kw('tempfiles'),
		_kw_terms_cache: ($) => kw('terms_cache'),
		_kw_terms_order: ($) => kw('terms_order'),
		_kw_then: ($) => kw('then'),
		_kw_throw: ($) => kw('throw'),
		_kw_timeout: ($) => kw('timeout'),
		_kw_to: ($) => kw('to'),
		_kw_token: ($) => kw('token'),
		_kw_tokenizers: ($) => kw('tokenizers'),
		_kw_trace: ($) => kw('trace'),
		_kw_transaction: ($) => kw('transaction'),
		_kw_type: ($) => kw('type'),
		_kw_unique: ($) => kw('unique'),
		_kw_unset: ($) => kw('unset'),
		_kw_update: ($) => kw('update'),
		_kw_upsert: ($) => kw('upsert'),
		_kw_url: ($) => kw('url'),
		_kw_use: ($) => kw('use'),
		_kw_user: ($) => kw('user'),
		_kw_value: ($) => kw('value'),
		_kw_values: ($) => kw('values'),
		_kw_version: ($) => kw('version'),
		_kw_when: ($) => kw('when'),
		_kw_where: ($) => kw('where'),
		_kw_with: ($) => kw('with'),

		// Operator keywords
		_kw_contains: ($) => kw('contains'),
		_kw_containsnot: ($) => kw('containsnot'),
		_kw_containsall: ($) => kw('containsall'),
		_kw_containsany: ($) => kw('containsany'),
		_kw_containsnone: ($) => kw('containsnone'),
		_kw_inside: ($) => kw('inside'),
		_kw_notinside: ($) => kw('notinside'),
		_kw_allinside: ($) => kw('allinside'),
		_kw_anyinside: ($) => kw('anyinside'),
		_kw_noneinside: ($) => kw('noneinside'),
		_kw_outside: ($) => kw('outside'),
		_kw_intersects: ($) => kw('intersects'),

		// Distance keywords
		_kw_chebyshev: ($) => kw('chebyshev'),
		_kw_cosine: ($) => kw('cosine'),
		_kw_cosine_normalized: ($) => kw('cosine_normalized'),
		_kw_inner_product: ($) => kw('inner_product'),
		_kw_euclidean: ($) => kw('euclidean'),
		_kw_hamming: ($) => kw('hamming'),
		_kw_jaccard: ($) => kw('jaccard'),
		_kw_manhattan: ($) => kw('manhattan'),
		_kw_minkowski: ($) => kw('minkowski'),
		_kw_pearson: ($) => kw('pearson'),

		// Analyzer Filter keywords
		_kw_ascii: ($) => kw('ascii'),
		_kw_edgengram: ($) => kw('edgengram'),
		_kw_ngram: ($) => kw('ngram'),
		_kw_snowball: ($) => kw('snowball'),
		_kw_uppercase: ($) => kw('uppercase'),
		_kw_lowercase: ($) => kw('lowercase'),

		// Analyzer Tokenizer keywords
		_kw_blank: ($) => kw('blank'),
		_kw_camel: ($) => kw('camel'),
		_kw_class: ($) => kw('class'),
		_kw_punct: ($) => kw('punct'),

		// Token type keywords
		_kw_jwks: ($) => kw('jwks'),
		_kw_eddsa: ($) => kw('eddsa'),
		_kw_es256: ($) => kw('es256'),
		_kw_es384: ($) => kw('es384'),
		_kw_es512: ($) => kw('es512'),
		_kw_hs256: ($) => kw('hs256'),
		_kw_hs384: ($) => kw('hs384'),
		_kw_hs512: ($) => kw('hs512'),
		_kw_ps256: ($) => kw('ps256'),
		_kw_ps384: ($) => kw('ps384'),
		_kw_ps512: ($) => kw('ps512'),
		_kw_rs256: ($) => kw('rs256'),
		_kw_rs384: ($) => kw('rs384'),
		_kw_rs512: ($) => kw('rs512'),

		// Index type keywords (f32/f64/i16/i32/i64)
		_kw_f16: ($) => kw('f16'),
		_kw_f32: ($) => kw('f32'),
		_kw_f64: ($) => kw('f64'),
		_kw_i16: ($) => kw('i16'),
		_kw_i32: ($) => kw('i32'),
		_kw_i64: ($) => kw('i64'),
		_kw_i8: ($) => kw('i8'),
		_kw_u8: ($) => kw('u8'),

		_kw_rand: ($) => kw('rand'),
		_kw_count: ($) => kw('count'),

		// Misc
		_kw_owner: ($) => kw('owner'),
		_kw_editor: ($) => kw('editor'),
		_kw_viewer: ($) => kw('viewer'),
		_kw_refresh: ($) => kw('refresh'),
		_kw_bearer: ($) => kw('bearer'),
		_kw_grant: ($) => kw('grant'),
		_kw_module: ($) => kw('module'),
		_kw_purge: ($) => kw('purge'),
		_kw_revoke: ($) => kw('revoke'),
		_kw_revoked: ($) => kw('revoked'),
		_kw_expired: ($) => kw('expired'),
		_kw_prepare: ($) => kw('prepare'),
		_kw_ratelimit: ($) => kw('ratelimit'),
		_kw_per: ($) => kw('per'),
		_kw_max: ($) => kw('max'),
		_kw_sequence: ($) => kw('sequence'),
		_kw_batch: ($) => kw('batch'),
		_kw_matches: ($) => kw('matches'),
		_kw_original: ($) => kw('original'),
		_kw_future: ($) => kw('future'),
		_kw_import: ($) => kw('import'),
		_kw_fulltext: ($) => kw('fulltext'),

		// Catch-all keyword union, used by visible Keyword rule
		_any_kw: ($) => choice(...KEYWORDS.map((name) => $[`_kw_${name}`])),
	},
});
