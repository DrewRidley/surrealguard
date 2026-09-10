//! Engine tests.
//!
//! Every case drives the real pipeline — analyze a schema plus a query, then
//! complete at a cursor — so a test failure means the feature is broken, not
//! that a fixture drifted. The cursor is written as `▏` in the query text and
//! stripped before parsing.

use std::collections::BTreeSet;

use super::*;
use crate::analysis::{analyze_workspace, Workspace};
use surrealguard_syntax::parse::parse_source;

/// A small but realistic schema: a schemafull table with scalar, optional,
/// link, and array-of-link fields; a second table to link to; a relation edge;
/// and a `fn::` function.
const SCHEMA: &str = r#"
DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD nickname ON person TYPE option<string>;
DEFINE FIELD status ON person TYPE string;
DEFINE FIELD age ON person TYPE int;
DEFINE FIELD employer ON person TYPE record<company>;
DEFINE FIELD tags ON person TYPE array<string>;
DEFINE FIELD address ON person TYPE object;
DEFINE FIELD address.city ON person TYPE string;

DEFINE TABLE company SCHEMAFULL;
DEFINE FIELD title ON company TYPE string;
DEFINE FIELD headcount ON company TYPE int;

DEFINE TABLE works_at SCHEMAFULL TYPE RELATION IN person OUT company;
DEFINE FIELD since ON works_at TYPE datetime;

DEFINE FUNCTION fn::shout($text: string) -> string { RETURN string::uppercase($text); };
DEFINE PARAM $tenant VALUE 'acme';
"#;

/// Analyzes `SCHEMA` plus `query` and completes where `▏` sits.
struct Fixture {
    output: AnalysisOutput,
    schema: SchemaIndex,
    parsed: ParsedSource,
    offset: u32,
}

impl Fixture {
    fn new(query: &str) -> Self {
        Self::with_schema(SCHEMA, query)
    }

    fn with_schema(schema_text: &str, query: &str) -> Self {
        let offset = query
            .find('▏')
            .expect("query fixtures must place the cursor with `▏`") as u32;
        let query = query.replace('▏', "");

        let mut workspace = Workspace::default();
        workspace.add_virtual_source("schema".into(), schema_text.to_string());
        let query_id = workspace.add_virtual_source("query".into(), query.clone());
        let analysis = analyze_workspace(&workspace);

        let parsed = parse_source(query_id.clone(), query).expect("query parses");
        Self {
            output: analysis
                .sources
                .get(&query_id)
                .cloned()
                .expect("query source is analyzed"),
            schema: analysis.schema,
            parsed,
            offset,
        }
    }

    fn context(&self) -> CompletionContext {
        completion_context_at(&self.output, &self.schema, &self.parsed, self.offset)
    }

    fn complete(&self) -> Vec<CompletionCandidate> {
        complete_at(&self.output, &self.schema, &self.parsed, self.offset)
    }

    fn labels(&self) -> Vec<String> {
        self.complete()
            .into_iter()
            .map(|candidate| candidate.label)
            .collect()
    }

    /// Labels of one candidate class, in ranked order.
    fn labels_of(&self, kind: CandidateKind) -> Vec<String> {
        self.complete()
            .into_iter()
            .filter(|candidate| candidate.kind == kind)
            .map(|candidate| candidate.label)
            .collect()
    }

    fn rank_of(&self, label: &str) -> Option<usize> {
        self.complete()
            .iter()
            .position(|candidate| candidate.label == label)
    }
}

fn context_kind(query: &str) -> ContextKind {
    Fixture::new(query).context().kind
}

// ---------------------------------------------------------------------------
// Context detection, including the inputs that do not parse
// ---------------------------------------------------------------------------

#[test]
fn a_projection_position_is_a_field_position_even_before_the_from_is_typed() {
    assert_eq!(context_kind("SELECT ▏"), ContextKind::FieldName);
    assert_eq!(
        context_kind("SELECT name, ▏ FROM person"),
        ContextKind::FieldName
    );
}

#[test]
fn an_empty_projection_before_an_existing_from_still_sees_its_table() {
    // tree-sitter collapses `SELECT  FROM person` into a single top-level
    // ERROR node holding one Keyword — every structural fact is gone. The
    // token scan still finds SELECT, FROM, and the table.
    let fixture = Fixture::new("SELECT ▏ FROM person");
    assert!(
        fixture.parsed.has_error(),
        "fixture must exercise broken input"
    );
    assert_eq!(fixture.context().kind, ContextKind::FieldName);
    assert_eq!(fixture.context().tables, vec!["person".to_string()]);
    let labels = fixture.labels_of(CandidateKind::Field);
    for expected in ["name", "status", "age", "employer", "tags", "id"] {
        assert!(
            labels.contains(&expected.to_string()),
            "missing {expected} in {labels:?}"
        );
    }
}

#[test]
fn a_dangling_from_is_a_table_position() {
    let fixture = Fixture::new("SELECT name FROM ▏");
    assert_eq!(fixture.context().kind, ContextKind::TableName);
    let tables = fixture.labels_of(CandidateKind::Table);
    assert!(tables.contains(&"person".to_string()));
    assert!(tables.contains(&"company".to_string()));
    assert!(tables.contains(&"works_at".to_string()));
}

#[test]
fn a_lone_dollar_after_a_comparison_is_a_param_position() {
    let fixture = Fixture::new("SELECT * FROM person WHERE status = $▏");
    assert!(
        fixture.parsed.has_error(),
        "fixture must exercise broken input"
    );
    let context = fixture.context();
    assert_eq!(context.kind, ContextKind::ParamName);
    assert_eq!(context.expected, Some(Kind::String));
}

#[test]
fn a_where_clause_head_is_a_field_position_and_its_right_hand_side_is_a_value() {
    assert_eq!(
        context_kind("SELECT * FROM person WHERE ▏"),
        ContextKind::FieldName
    );
    assert_eq!(
        context_kind("SELECT * FROM person WHERE status = ▏"),
        ContextKind::Value
    );
    // A second conjunct is a field position again.
    assert_eq!(
        context_kind("SELECT * FROM person WHERE age > 3 AND ▏"),
        ContextKind::FieldName
    );
}

#[test]
fn a_dot_is_a_member_position_resolved_through_the_receivers_kind() {
    let fixture = Fixture::new("SELECT employer.▏ FROM person");
    let context = fixture.context();
    assert_eq!(context.kind, ContextKind::Member);
    assert_eq!(
        context.receiver,
        Some(Kind::Record(vec![surrealdb_types::Table::from("company")]))
    );
    let members = fixture.labels_of(CandidateKind::Field);
    assert!(members.contains(&"title".to_string()));
    assert!(members.contains(&"headcount".to_string()));
    // The link target's fields, not the row table's.
    assert!(!members.contains(&"nickname".to_string()));
}

#[test]
fn a_destructure_offers_the_receivers_members_and_omits_the_ones_already_written() {
    let fixture = Fixture::new("SELECT employer.{ title, ▏ } FROM person");
    let context = fixture.context();
    assert_eq!(context.kind, ContextKind::Destructure);
    assert!(context.exclude.contains("title"));
    let members = fixture.labels_of(CandidateKind::Field);
    assert!(members.contains(&"headcount".to_string()));
    assert!(!members.contains(&"title".to_string()));
}

#[test]
fn a_partial_function_path_completes_within_its_family() {
    let fixture = Fixture::new("SELECT string::u▏ FROM person");
    assert_eq!(fixture.context().kind, ContextKind::FunctionPath);
    let functions = fixture.labels_of(CandidateKind::Function);
    assert!(functions.contains(&"string::uppercase".to_string()));
    assert!(!functions.contains(&"math::abs".to_string()));
}

#[test]
fn workspace_functions_are_offered_and_outrank_a_same_named_builtin_family() {
    let fixture = Fixture::new("SELECT fn::▏ FROM person");
    let functions = fixture.labels_of(CandidateKind::Function);
    assert_eq!(functions, vec!["fn::shout".to_string()]);
}

#[test]
fn a_content_object_offers_writable_keys_only() {
    let fixture = Fixture::new("CREATE person CONTENT { name: 'a', ▏ }");
    let context = fixture.context();
    assert_eq!(context.kind, ContextKind::ObjectKey);
    assert!(context.exclude.contains("name"));
    let keys = fixture.labels_of(CandidateKind::ObjectKey);
    assert!(keys.contains(&"status".to_string()));
    assert!(keys.contains(&"employer".to_string()));
    // Already written, and the never-writable implicit `id`.
    assert!(!keys.contains(&"name".to_string()));
    assert!(!keys.contains(&"id".to_string()));
}

#[test]
fn an_object_value_position_takes_its_expectation_from_the_key() {
    let context = Fixture::new("CREATE person CONTENT { age: ▏ }").context();
    assert_eq!(context.kind, ContextKind::Value);
    assert_eq!(context.expected, Some(Kind::Int));
}

#[test]
fn a_mutation_target_and_its_set_clause_classify_separately() {
    assert_eq!(context_kind("CREATE ▏"), ContextKind::TableName);
    assert_eq!(context_kind("UPDATE person SET ▏"), ContextKind::FieldName);
    assert_eq!(context_kind("DELETE ▏"), ContextKind::TableName);
    assert_eq!(context_kind("INSERT INTO ▏"), ContextKind::TableName);
}

#[test]
fn a_graph_step_alternates_an_edge_slot_and_the_node_it_lands_on() {
    assert_eq!(context_kind("RELATE $a->▏"), ContextKind::EdgeTable);
    assert_eq!(
        context_kind("SELECT ->▏ FROM person"),
        ContextKind::EdgeTable
    );
    assert_eq!(
        context_kind("SELECT ->works_at->▏ FROM person"),
        ContextKind::GraphNode
    );
    assert_eq!(
        context_kind("SELECT ->works_at->company->▏ FROM person"),
        ContextKind::EdgeTable
    );
}

#[test]
fn a_define_on_clause_is_a_table_position_and_its_fields_clause_a_field_position() {
    assert_eq!(context_kind("DEFINE FIELD x ON ▏"), ContextKind::TableName);
    let fixture = Fixture::new("DEFINE INDEX i ON person FIELDS ▏");
    assert_eq!(fixture.context().kind, ContextKind::FieldName);
    assert!(fixture
        .labels_of(CandidateKind::Field)
        .contains(&"name".to_string()));
}

#[test]
fn a_let_value_is_a_value_position() {
    assert_eq!(context_kind("LET $x = ▏"), ContextKind::Value);
}

#[test]
fn a_subquery_inside_a_call_classifies_on_its_own_terms() {
    assert_eq!(
        context_kind("SELECT array::len((SELECT name FROM ▏)) FROM person"),
        ContextKind::TableName
    );
}

#[test]
fn a_position_inside_a_string_or_comment_offers_nothing() {
    let fixture = Fixture::new("SELECT * FROM person WHERE name = 'fr▏om'");
    assert!(!fixture.context().enabled);
    assert!(fixture.complete().is_empty());

    let fixture = Fixture::new("-- pick a ta▏ble\nSELECT * FROM person");
    assert!(fixture.complete().is_empty());
}

#[test]
fn a_member_position_whose_receiver_has_no_known_type_offers_nothing_rather_than_guessing() {
    // `unknown_thing` is not a field or a table, so its members are unknown.
    // Offering the row table's fields (or tables, or params) here would be
    // confidently wrong.
    let fixture = Fixture::new("SELECT unknown_thing.▏ FROM person");
    assert_eq!(fixture.context().kind, ContextKind::Member);
    assert!(fixture.complete().is_empty());
}

// ---------------------------------------------------------------------------
// Graph slots: what a step can *actually* traverse
// ---------------------------------------------------------------------------

/// The schema from the report that produced these bugs: five node tables and
/// five edges, only two of which leave an `account`.
const GRAPH: &str = r#"
DEFINE TABLE account SCHEMAFULL;
DEFINE FIELD owner ON account TYPE option<record<account>>;
DEFINE TABLE email_address SCHEMAFULL;
DEFINE TABLE organization SCHEMAFULL;
DEFINE TABLE keyring SCHEMAFULL;
DEFINE TABLE file SCHEMAFULL;
DEFINE TABLE has_email SCHEMAFULL TYPE RELATION FROM account TO email_address;
DEFINE FIELD verified ON has_email TYPE bool;
DEFINE TABLE employee_of SCHEMAFULL TYPE RELATION FROM account TO organization;
DEFINE TABLE keyring_wraps SCHEMAFULL TYPE RELATION FROM keyring TO account;
DEFINE TABLE secure_entity SCHEMAFULL TYPE RELATION FROM file TO account;
DEFINE TABLE has_subsidiary SCHEMAFULL TYPE RELATION FROM organization TO organization;
"#;

fn graph(query: &str) -> Fixture {
    Fixture::with_schema(GRAPH, query)
}

/// Asserts the offered labels are exactly `expected`, in any order — the
/// point of these cases is the *set*, not the ranking.
fn assert_offers(fixture: &Fixture, expected: &[&str]) {
    let mut labels = fixture.labels();
    labels.sort();
    let mut wanted: Vec<String> = expected.iter().map(ToString::to_string).collect();
    wanted.sort();
    assert_eq!(labels, wanted);
}

#[test]
fn a_forward_edge_slot_offers_only_edges_that_leave_the_receiver() {
    let fixture = graph("SELECT ->▏ FROM account");
    // `account` is the IN of exactly these two.
    assert_offers(&fixture, &["employee_of", "has_email"]);
    // The edges that only *arrive* at an account, the one that never touches
    // one, the plain tables, and the params are all invalid here.
    for absent in [
        "keyring_wraps",
        "secure_entity",
        "has_subsidiary",
        "account",
        "email_address",
        "file",
        "keyring",
        "organization",
        "$auth",
        "$session",
        "$access",
    ] {
        assert!(
            !fixture.labels().contains(&absent.to_string()),
            "`{absent}` is not traversable forward from an `account`"
        );
    }
}

#[test]
fn a_backward_edge_slot_offers_only_edges_that_arrive_at_the_receiver() {
    let fixture = graph("SELECT <-▏ FROM account");
    assert_offers(&fixture, &["keyring_wraps", "secure_entity"]);
    // The forward-only pair must not appear in the backward slot.
    for absent in ["has_email", "employee_of", "has_subsidiary"] {
        assert!(!fixture.labels().contains(&absent.to_string()), "{absent}");
    }
}

#[test]
fn a_node_slot_offers_only_the_traversed_edges_far_endpoint() {
    let fixture = graph("SELECT ->has_email->▏ FROM account");
    assert_offers(&fixture, &["email_address"]);

    // Backwards, the far endpoint is the other end.
    let backward = graph("SELECT <-keyring_wraps->▏ FROM account");
    assert_offers(&backward, &["keyring"]);
}

#[test]
fn a_second_hop_filters_by_the_table_the_first_hop_reached() {
    // `has_email` runs account -> email_address, so standing on an
    // `email_address` there is nothing to traverse forward.
    let fixture = graph("SELECT ->has_email->email_address->▏ FROM account");
    assert_eq!(fixture.context().kind, ContextKind::EdgeTable);
    assert!(
        fixture.complete().is_empty(),
        "no edge leaves an `email_address`: {:?}",
        fixture.labels()
    );

    // …and from an `organization` the subsidiary edge is the only one.
    let onward = graph("SELECT ->employee_of->organization->▏ FROM account");
    assert_offers(&onward, &["has_subsidiary"]);
}

#[test]
fn a_typed_param_receiver_is_resolved_through_its_record_kind() {
    // The reported case: `$value` holds a `record<email_address>`, so
    // `has_email` — which runs account -> email_address — is not traversable
    // forward from it.
    let fixture = Fixture::with_schema(
        GRAPH,
        "DEFINE FIELD mail ON email_address TYPE record<email_address> VALUE $value->▏;",
    );
    assert_eq!(fixture.context().kind, ContextKind::EdgeTable);
    assert!(
        !fixture.labels().contains(&"has_email".to_string()),
        "an email_address is the OUT of has_email, not its IN: {:?}",
        fixture.labels()
    );

    // A `LET` binding resolves the same way, through `option`/`array` too.
    let bound = Fixture::with_schema(
        GRAPH,
        "LET $mail = (SELECT * FROM ONLY email_address:x); SELECT * FROM account WHERE owner->▏",
    );
    assert_eq!(bound.context().kind, ContextKind::EdgeTable);
    let optional_link = graph("SELECT * FROM account WHERE owner->▏");
    // `owner` is `option<record<account>>`; the wrappers must not hide the
    // account underneath.
    assert_offers(&optional_link, &["employee_of", "has_email"]);
}

#[test]
fn this_inside_a_define_body_resolves_to_the_table_the_define_is_on() {
    let fixture = Fixture::with_schema(GRAPH, "DEFINE FIELD x ON account VALUE $this->▏;");
    assert_eq!(fixture.context().kind, ContextKind::EdgeTable);
    assert_offers(&fixture, &["employee_of", "has_email"]);

    let event = Fixture::with_schema(
        GRAPH,
        "DEFINE EVENT e ON keyring WHEN $event = 'CREATE' THEN { LET $x = $this->▏; };",
    );
    assert_offers(&event, &["keyring_wraps"]);
}

#[test]
fn a_record_id_literal_is_a_receiver_and_a_relate_target_is_filtered_by_it() {
    let fixture = graph("RELATE account:alice->▏");
    assert_eq!(fixture.context().kind, ContextKind::EdgeTable);
    assert_offers(&fixture, &["employee_of", "has_email"]);
}

#[test]
fn an_unresolvable_receiver_offers_nothing_rather_than_every_edge() {
    // `$a` has no known kind, so which edges leave it is unknowable —
    // and a wrong edge is worse than no suggestion.
    let fixture = graph("RELATE $a->▏");
    assert_eq!(fixture.context().kind, ContextKind::EdgeTable);
    assert!(fixture.complete().is_empty(), "{:?}", fixture.labels());

    // A chain through an unknown table is unresolvable from there on.
    let unknown = graph("SELECT ->not_a_table->▏ FROM account");
    assert!(unknown.complete().is_empty(), "{:?}", unknown.labels());

    // So is a chain whose written edge does not connect the receiver.
    let broken = graph("SELECT ->has_subsidiary->▏ FROM account");
    assert!(broken.complete().is_empty(), "{:?}", broken.labels());
}

#[test]
fn a_step_filter_does_not_break_the_chain_that_follows_it() {
    let fixture = graph("SELECT ->has_email[WHERE verified = true]->▏ FROM account");
    assert_eq!(fixture.context().kind, ContextKind::GraphNode);
    assert_offers(&fixture, &["email_address"]);
}

// ---------------------------------------------------------------------------
// A graph step's own filter runs on the step's table
// ---------------------------------------------------------------------------

#[test]
fn a_graph_step_filter_completes_the_edges_fields_not_the_outer_rows() {
    for query in [
        "SELECT ->has_email[WHERE ▏]->email_address FROM account",
        "SELECT ->has_email[? ▏]->email_address FROM account",
        "SELECT ->(has_email WHERE ▏)->email_address FROM account",
    ] {
        let fixture = graph(query);
        assert_eq!(
            fixture.context().tables,
            vec!["has_email".to_string()],
            "{query}"
        );
        let fields = fixture.labels_of(CandidateKind::Field);
        for expected in ["verified", "in", "out", "id"] {
            assert!(
                fields.contains(&expected.to_string()),
                "{query}: {fields:?}"
            );
        }
        // The outer row's own fields belong to `account`, not to the edge.
        assert!(
            !fields.contains(&"owner".to_string()),
            "{query}: {fields:?}"
        );
    }
}

#[test]
fn a_graph_step_filters_right_hand_side_is_typed_by_the_edges_field() {
    let fixture = graph("SELECT ->has_email[WHERE verified = ▏]->email_address FROM account");
    assert_eq!(fixture.context().expected, Some(Kind::Bool));
}

// ---------------------------------------------------------------------------
// GROUP BY
// ---------------------------------------------------------------------------

const GROUPED: &str = r#"
DEFINE TABLE sale SCHEMAFULL;
DEFINE FIELD region ON sale TYPE string;
DEFINE FIELD amount ON sale TYPE int;
DEFINE INDEX sale_region ON sale FIELDS region;
DEFINE INDEX sale_amount ON sale FIELDS amount UNIQUE;
DEFINE TABLE other SCHEMAFULL;
DEFINE INDEX other_only ON other FIELDS id;
"#;

#[test]
fn a_group_key_offers_the_projections_aliases_and_the_rows_fields_only() {
    let fixture = Fixture::with_schema(
        GROUPED,
        "SELECT region, math::sum(amount) AS total FROM sale GROUP BY ▏",
    );
    assert_eq!(fixture.context().kind, ContextKind::GroupKey);
    let labels = fixture.labels();
    for expected in ["region", "amount", "id", "total"] {
        assert!(
            labels.contains(&expected.to_string()),
            "missing {expected}: {labels:?}"
        );
    }
    // Nothing that cannot label a group.
    for absent in ["math::sum", "math::", "$auth", "$session", "sale", "other"] {
        assert!(
            !labels.contains(&absent.to_string()),
            "{absent} in {labels:?}"
        );
    }
}

#[test]
fn a_second_group_key_does_not_re_offer_the_first() {
    let fixture = Fixture::with_schema(
        GROUPED,
        "SELECT region, amount FROM sale GROUP BY region, ▏",
    );
    let labels = fixture.labels();
    assert!(labels.contains(&"amount".to_string()), "{labels:?}");
    assert!(!labels.contains(&"region".to_string()), "{labels:?}");
}

#[test]
fn order_by_stays_a_plain_field_position() {
    // `BY` belongs to whichever clause opened it; ORDER must not be dragged
    // into the group-key rules.
    assert_eq!(
        Fixture::with_schema(GROUPED, "SELECT * FROM sale ORDER BY ▏")
            .context()
            .kind,
        ContextKind::FieldName
    );
}

// ---------------------------------------------------------------------------
// Indexes
// ---------------------------------------------------------------------------

#[test]
fn with_index_offers_the_queried_tables_indexes_and_nothing_else() {
    let fixture = Fixture::with_schema(
        GROUPED,
        "SELECT * FROM sale WITH INDEX ▏ WHERE region = 'a'",
    );
    assert_eq!(fixture.context().kind, ContextKind::IndexName);
    assert_offers(&fixture, &["sale_region", "sale_amount"]);
    let labels = fixture.labels();
    // Another table's index, the tables themselves, and the params are all
    // invalid here.
    for absent in ["other_only", "sale", "other", "$auth", "region"] {
        assert!(
            !labels.contains(&absent.to_string()),
            "{absent} in {labels:?}"
        );
    }
    let index = fixture
        .complete()
        .into_iter()
        .find(|candidate| candidate.label == "sale_amount")
        .expect("the index is offered");
    assert_eq!(index.kind, CandidateKind::Index);
    assert_eq!(index.detail.as_deref(), Some("unique index on amount"));
}

#[test]
fn defining_an_index_is_still_a_name_position_not_an_index_reference() {
    // `DEFINE INDEX` also puts `INDEX` in the token stream; only the `WITH`
    // pair is a reference to an existing index.
    assert_ne!(
        Fixture::with_schema(GROUPED, "DEFINE INDEX ▏")
            .context()
            .kind,
        ContextKind::IndexName
    );
    assert_eq!(
        Fixture::with_schema(GROUPED, "DEFINE INDEX i ON sale FIELDS ▏")
            .context()
            .kind,
        ContextKind::FieldName
    );
}

#[test]
fn the_index_backed_search_builtins_are_offered_only_where_an_index_backs_them() {
    let indexed = r#"
DEFINE ANALYZER english TOKENIZERS class;
DEFINE TABLE article SCHEMAFULL;
DEFINE FIELD body ON article TYPE string;
DEFINE INDEX article_body ON article FIELDS body SEARCH ANALYZER english BM25 HIGHLIGHTS;
DEFINE TABLE note SCHEMAFULL;
DEFINE FIELD body ON note TYPE string;
"#;
    let with_index = Fixture::with_schema(indexed, "SELECT search::▏ FROM article");
    let labels = with_index.labels();
    assert!(labels.contains(&"search::score".to_string()), "{labels:?}");
    assert!(
        labels.contains(&"search::highlight".to_string()),
        "{labels:?}"
    );

    let without = Fixture::with_schema(indexed, "SELECT search::▏ FROM note");
    let labels = without.labels();
    for absent in ["search::score", "search::highlight", "search::offsets"] {
        assert!(
            !labels.contains(&absent.to_string()),
            "`{absent}` needs a full-text index on `note`, and there is none: {labels:?}"
        );
    }
    // The rest of the family works without one.
    assert!(
        labels.contains(&"search::analyze".to_string()),
        "{labels:?}"
    );
}

// ---------------------------------------------------------------------------
// Ranking
// ---------------------------------------------------------------------------

#[test]
fn type_compatibility_outranks_a_closer_spelling() {
    // Both params start with `st`; only one is a `string` like `status`.
    let fixture = Fixture::with_schema(
        SCHEMA,
        "LET $stamp = 12; LET $st_value = 'open'; SELECT * FROM person WHERE status = $▏",
    );
    let context = fixture.context();
    assert_eq!(context.expected, Some(Kind::String));
    let params = fixture.labels_of(CandidateKind::Param);
    let string_param = params
        .iter()
        .position(|label| label == "$st_value")
        .expect("the string param is offered");
    let int_param = params
        .iter()
        .position(|label| label == "$stamp")
        .expect("the int param is offered");
    assert!(
        string_param < int_param,
        "the type-compatible param must rank first, got {params:?}"
    );
}

#[test]
fn a_typed_prefix_still_leads_when_both_candidates_fit_the_type() {
    let fixture = Fixture::new("SELECT * FROM person WHERE sta▏");
    let fields = fixture.labels_of(CandidateKind::Field);
    assert_eq!(fields.first().map(String::as_str), Some("status"));
}

#[test]
fn an_exact_prefix_beats_a_scattered_subsequence() {
    assert!(match_score("na", "name").unwrap() > match_score("na", "nickname").unwrap());
    // A hit at a word boundary beats the same characters scattered.
    assert!(match_score("len", "string::len").unwrap() > match_score("len", "silent").unwrap());
    assert_eq!(match_score("zzz", "name"), None);
    // A case-exact prefix edges out a case-insensitive one.
    assert!(match_score("na", "name").unwrap() > match_score("Na", "name").unwrap());
}

#[test]
fn an_unknown_type_scores_neutrally_rather_than_as_a_mismatch() {
    let compatible = type_score(Some(&Kind::String), Some(&Kind::String));
    let unknown = type_score(None, Some(&Kind::String));
    let mismatch = type_score(Some(&Kind::Int), Some(&Kind::String));
    assert!(compatible > unknown, "a known fit beats an unknown type");
    assert!(
        unknown > mismatch,
        "an unknown type beats a proven mismatch"
    );
    // No expectation at all is neutral for everyone.
    assert_eq!(type_score(Some(&Kind::Int), None), type_score(None, None));
}

#[test]
fn the_ranked_order_is_pinned_for_the_client_by_sort_text() {
    let fixture = Fixture::new("SELECT ▏ FROM person");
    let candidates = fixture.complete();
    assert!(candidates.len() > 1);
    let sort_texts: Vec<&str> = candidates
        .iter()
        .map(|candidate| candidate.sort_text.as_str())
        .collect();
    let mut sorted = sort_texts.clone();
    sorted.sort_unstable();
    assert_eq!(sort_texts, sorted, "sort_text must follow the ranked order");
    assert_eq!(sort_texts[0], "0000");
}

#[test]
fn a_schema_field_outranks_a_builtin_that_matches_the_same_prefix() {
    let fixture = Fixture::new("SELECT ta▏ FROM person");
    let field = fixture
        .rank_of("tags")
        .expect("the row table's field is offered");
    let builtin = fixture
        .rank_of("type::table")
        .expect("a built-in matching `ta` is offered too");
    assert!(
        field < builtin,
        "the row table's field must beat a built-in: {:?}",
        fixture.labels()
    );
}

#[test]
fn every_item_replaces_the_whole_token_being_typed() {
    let fixture = Fixture::new("SELECT nam▏e FROM person");
    let candidate = fixture
        .complete()
        .into_iter()
        .find(|candidate| candidate.label == "name")
        .expect("`name` is offered");
    // The range covers `name`, not just the typed `nam`, so accepting the
    // item does not leave a stray `e`.
    assert_eq!(candidate.replace, (7, 11));
}

// ---------------------------------------------------------------------------
// Params
// ---------------------------------------------------------------------------

#[test]
fn params_in_scope_include_bindings_host_params_and_define_param() {
    let fixture = Fixture::new("LET $local = 1; SELECT * FROM person WHERE age = $▏");
    let params = fixture.labels_of(CandidateKind::Param);
    assert!(params.contains(&"$local".to_string()), "{params:?}");
    assert!(params.contains(&"$tenant".to_string()), "{params:?}");
}

#[test]
fn a_binding_is_not_offered_before_it_is_written_or_after_its_block_closed() {
    let after = Fixture::new("LET $early = 1; SELECT $▏");
    assert!(after
        .labels_of(CandidateKind::Param)
        .contains(&"$early".to_string()));

    let before = Fixture::new("SELECT $▏; LET $late = 1;");
    assert!(!before
        .labels_of(CandidateKind::Param)
        .contains(&"$late".to_string()));

    let closed = Fixture::new("DEFINE FUNCTION fn::f() { LET $inner = 1; RETURN 1; }; SELECT $▏");
    assert!(
        !closed
            .labels_of(CandidateKind::Param)
            .contains(&"$inner".to_string()),
        "a binding in a closed block is out of scope"
    );
}

#[test]
fn context_params_of_the_enclosing_define_are_offered_with_their_kinds() {
    let fixture = Fixture::with_schema(
        SCHEMA,
        "DEFINE EVENT e ON person WHEN $event = 'CREATE' THEN { CREATE company SET title = $▏ };",
    );
    let candidates = fixture.complete();
    let after = candidates
        .iter()
        .find(|candidate| candidate.label == "$after")
        .expect("`$after` is bound by the enclosing DEFINE EVENT");
    assert_eq!(after.detail.as_deref(), Some("record<person>"));
}

// ---------------------------------------------------------------------------
// Members through wrappers and links
// ---------------------------------------------------------------------------

#[test]
fn members_resolve_through_option_and_array_wrappers_and_across_links() {
    let schema = r#"
DEFINE TABLE team SCHEMAFULL;
DEFINE FIELD lead ON team TYPE option<record<person>>;
DEFINE FIELD members ON team TYPE array<record<person>>;
DEFINE TABLE person SCHEMAFULL;
DEFINE FIELD name ON person TYPE string;
DEFINE FIELD age ON person TYPE int;
"#;
    for query in ["SELECT lead.▏ FROM team", "SELECT members.▏ FROM team"] {
        let fixture = Fixture::with_schema(schema, query);
        let members = fixture.labels_of(CandidateKind::Field);
        assert!(
            members.contains(&"name".to_string()),
            "{query}: {members:?}"
        );
        assert!(members.contains(&"age".to_string()), "{query}: {members:?}");
    }
}

#[test]
fn a_relation_edge_offers_its_implicit_in_and_out() {
    let fixture = Fixture::new("SELECT ▏ FROM works_at");
    let fields = fixture.labels_of(CandidateKind::Field);
    for expected in ["in", "out", "id", "since"] {
        assert!(
            fields.contains(&expected.to_string()),
            "missing {expected}: {fields:?}"
        );
    }
}

#[test]
fn a_typed_receiver_offers_the_methods_its_kind_family_dispatches() {
    let fixture = Fixture::new("SELECT tags.▏ FROM person");
    let methods = fixture.labels_of(CandidateKind::Method);
    assert!(methods.contains(&"len".to_string()), "{methods:?}");
    assert!(methods.contains(&"distinct".to_string()), "{methods:?}");
    // Dispatch is by kind family: a string method is not on an array.
    assert!(!methods.contains(&"uppercase".to_string()));
}

// ---------------------------------------------------------------------------
// Expectations from call signatures
// ---------------------------------------------------------------------------

#[test]
fn a_call_argument_takes_its_expectation_from_the_signature() {
    let workspace_call = Fixture::new("SELECT fn::shout(▏) FROM person");
    assert_eq!(workspace_call.context().expected, Some(Kind::String));

    let builtin_call = Fixture::new("SELECT string::len(▏) FROM person");
    assert_eq!(builtin_call.context().expected, Some(Kind::String));

    // …and it ranks the fitting field first.
    let fields = builtin_call.labels_of(CandidateKind::Field);
    let name = fields.iter().position(|label| label == "name");
    let age = fields.iter().position(|label| label == "age");
    assert!(
        name < age,
        "the string field must outrank the int one: {fields:?}"
    );
}

// ---------------------------------------------------------------------------
// Robustness
// ---------------------------------------------------------------------------

#[test]
fn completing_anywhere_in_pathological_input_never_panics_and_never_invents_a_name() {
    let sources = [
        "",
        "S",
        "SELECT",
        "SELECT * FROM",
        "SELECT .{ FROM",
        "CREATE person CONTENT {",
        "LET $x = (((",
        "$",
        "..",
        "SELECT $$$ FROM ]]]",
        "DEFINE FIELD ON ON ON",
        "SELECT ⟨wéird⟩.  FROM person",
        "/* unterminated",
        "'unterminated",
        "SELECT a->b->c-> FROM person",
    ];
    let known: BTreeSet<String> = {
        let fixture = Fixture::new("▏");
        fixture
            .schema
            .tables
            .keys()
            .cloned()
            .chain(fixture.schema.functions.keys().cloned())
            .collect()
    };

    for source in sources {
        let mut workspace = Workspace::default();
        workspace.add_virtual_source("schema".into(), SCHEMA.to_string());
        let query_id = workspace.add_virtual_source("query".into(), source.to_string());
        let analysis = analyze_workspace(&workspace);
        let output = analysis.sources.get(&query_id).cloned().unwrap_or_default();
        let parsed = parse_source(query_id, source).expect("parses");
        for offset in 0..=source.len() as u32 {
            if !source.is_char_boundary(offset as usize) {
                continue;
            }
            for candidate in complete_at(&output, &analysis.schema, &parsed, offset) {
                // A table candidate must be a table that exists.
                if candidate.kind == CandidateKind::Table {
                    assert!(
                        known.contains(&candidate.label),
                        "invented table `{}` at {offset} in {source:?}",
                        candidate.label
                    );
                }
                assert!(!candidate.label.is_empty());
            }
        }
    }
}

#[test]
fn builtins_are_rendered_from_the_analyzer_catalog() {
    // Completion has no catalog of its own: every offered built-in is a row of
    // the analyzer's dispatch table, rendered with the crate's kind renderer,
    // and every row the analyzer dispatches under a current, documented name
    // is offered. The two cannot drift because there is only one table.
    let catalog = crate::analyzer::function::builtin_catalog();
    let offered: BTreeSet<&str> = builtins::offered().map(|entry| entry.name).collect();
    assert!(
        offered.len() > 400,
        "expected the whole builtin surface, saw {}",
        offered.len()
    );
    for entry in catalog {
        let expected = entry.is_current() && entry.is_documented();
        assert_eq!(
            offered.contains(entry.name),
            expected,
            "`{}` offered={} but current={} documented={}",
            entry.name,
            !expected,
            entry.is_current(),
            entry.is_documented()
        );
        // The detail is `name(params) -> kind`, spelled by `render_kind`.
        let detail = builtins::signature_text(entry);
        assert!(
            detail.starts_with(&format!("{}(", entry.name)) && detail.contains(") -> "),
            "malformed detail for `{}`: {detail}",
            entry.name
        );
        assert!(
            crate::analyzer::function::is_builtin(entry.name),
            "`{}` is in the catalog but not a builtin",
            entry.name
        );
    }
    // A spelling a release removed is dispatched (for the rename hint) but
    // never offered; the current spelling is.
    assert!(!offered.contains("duration::from::days"));
    assert!(offered.contains("duration::from_days"));
    assert!(!offered.contains("count::count"));
    assert!(offered.contains("count"));
}

#[test]
fn builtin_signatures_render_with_the_crates_kind_spelling() {
    let detail = |name: &str| {
        builtins::signature_text(crate::analyzer::function::builtin(name).expect("a builtin"))
    };
    assert_eq!(detail("string::len"), "string::len(string) -> int");
    assert_eq!(detail("math::sum"), "math::sum(array<any>) -> number");
    // Optional parameters carry `?`; a variadic tail is `...`.
    assert_eq!(detail("rand::int"), "rand::int(number?, number?) -> int");
    assert_eq!(detail("rand::enum"), "rand::enum(any, ...) -> any");
    // Closure-taking functions show the closure parameter.
    assert_eq!(
        detail("array::map"),
        "array::map(array<any>, function) -> array<any>"
    );
    // A pass-through return is spelled as the parameter it passes through;
    // an element return the signature cannot name is `any`.
    assert_eq!(
        detail("array::add"),
        "array::add(array<any>, any) -> array<any>"
    );
    assert_eq!(detail("array::first"), "array::first(array<any>) -> any");
}

#[test]
fn a_method_candidate_ranks_by_the_return_kind_the_receiver_decides() {
    // `array::first` returns the element kind — unknowable in the flat list,
    // known once a receiver is in hand.
    let first = crate::analyzer::function::builtin("array::first").expect("a builtin");
    assert_eq!(builtins::return_kind(first), None);
    assert_eq!(
        builtins::method_return_kind(first, &Kind::Array(Box::new(Kind::String), None)),
        Some(Kind::String)
    );
    let len = crate::analyzer::function::builtin("array::len").expect("a builtin");
    assert_eq!(
        builtins::method_return_kind(len, &Kind::Array(Box::new(Kind::String), None)),
        Some(Kind::Int)
    );
    // A call argument's expectation comes from the same signature.
    let repeat = crate::analyzer::function::builtin("string::repeat").expect("a builtin");
    assert_eq!(builtins::parameter_kind(repeat, 0), Some(Kind::String));
    assert_eq!(builtins::parameter_kind(repeat, 5), None);
}

// ---------------------------------------------------------------------------
// Completion and hover read the same analysis
// ---------------------------------------------------------------------------

#[test]
fn completion_sees_the_narrowing_hover_already_saw() {
    // A guard that narrowed a symbol narrowed it for the completion list too.
    // Hover has read `output.narrowings` since they were recorded; completion
    // read the *binding's* kind, so past a diverging NONE-guard the `$p`
    // candidate was still labelled `none | { ... }` while hovering the same
    // name one line up said `{ ... }`. One analysis, two answers.
    let detail = |query: &str| -> String {
        let fixture = Fixture::new(query);
        fixture
            .complete()
            .into_iter()
            .find(|candidate| candidate.label == "$p")
            .and_then(|candidate| candidate.detail)
            .unwrap_or_default()
    };

    let before = detail(
        "LET $p = (SELECT * FROM ONLY person LIMIT 1);\n\
         RETURN $▏;",
    );
    assert!(
        before.starts_with("option<"),
        "before any guard the binding is optional: {before}"
    );

    let after = detail(
        "LET $p = (SELECT * FROM ONLY person LIMIT 1);\n\
         IF $p = NONE THEN THROW 'missing' END;\n\
         RETURN $▏;",
    );
    assert!(
        !after.starts_with("option<"),
        "past the guard `$p` cannot be NONE — hover already says so: {after}"
    );
}
