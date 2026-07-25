//! Mapping engine candidates onto LSP completion items.
//!
//! The ranking itself lives in `surrealguard_workspace::completion`; this
//! module only translates. Two translation choices carry weight:
//!
//! * **`sort_text` is passed through verbatim.** Without it a client re-sorts
//!   alphabetically and the whole type-aware ranking is thrown away.
//! * **Every item carries an explicit `text_edit`** over the range the engine
//!   computed, rather than relying on the client's idea of the "current
//!   word". A SurrealQL `$param` or `fn::name` spans characters most clients
//!   treat as word boundaries, so client-side range inference would leave
//!   `$` or `fn::` behind.

use tower_lsp::lsp_types::{
    CompletionItem, CompletionItemKind, CompletionItemLabelDetails, Documentation, TextEdit,
};

use surrealguard_workspace::{CandidateKind, CompletionCandidate};

/// Converts one ranked candidate into the item the editor shows.
pub fn candidate_to_item(text: &str, candidate: CompletionCandidate) -> CompletionItem {
    let (start, end) = candidate.replace;
    let range = crate::text::byte_range_to_lsp(text, start as usize, end as usize);

    CompletionItem {
        label: candidate.label.clone(),
        kind: Some(item_kind(candidate.kind)),
        detail: candidate.detail.clone(),
        label_details: Some(CompletionItemLabelDetails {
            detail: candidate.detail.clone().map(|detail| format!(" {detail}")),
            description: candidate.documentation.clone(),
        }),
        documentation: candidate
            .documentation
            .map(Documentation::String),
        sort_text: Some(candidate.sort_text),
        // Clients filter the list themselves as typing continues; they must
        // filter on the whole label, not on the edit's replaced text.
        filter_text: Some(candidate.label),
        text_edit: Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(TextEdit {
            range,
            new_text: candidate.insert_text,
        })),
        ..CompletionItem::default()
    }
}

/// The icon class an editor draws. A namespace is a `MODULE` so it reads as a
/// container the user steps into, and a method is `METHOD` rather than
/// `FUNCTION` so call sugar is visually distinct from a `fn::` call.
fn item_kind(kind: CandidateKind) -> CompletionItemKind {
    match kind {
        CandidateKind::Field => CompletionItemKind::FIELD,
        CandidateKind::Table => CompletionItemKind::CLASS,
        CandidateKind::Param => CompletionItemKind::VARIABLE,
        CandidateKind::Function => CompletionItemKind::FUNCTION,
        CandidateKind::Method => CompletionItemKind::METHOD,
        CandidateKind::Namespace => CompletionItemKind::MODULE,
        CandidateKind::ObjectKey => CompletionItemKind::PROPERTY,
        CandidateKind::Index => CompletionItemKind::REFERENCE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate() -> CompletionCandidate {
        CompletionCandidate {
            label: "$tenant".to_string(),
            insert_text: "$tenant".to_string(),
            kind: CandidateKind::Param,
            detail: Some("string".to_string()),
            documentation: Some("DEFINE PARAM".to_string()),
            candidate_kind: None,
            score: 0.9,
            sort_text: "0003".to_string(),
            replace: (14, 21),
        }
    }

    #[test]
    fn an_item_edits_the_engines_range_and_keeps_the_engines_order() {
        let text = "SELECT * FROM $tenant";
        let item = candidate_to_item(text, candidate());

        assert_eq!(item.sort_text.as_deref(), Some("0003"));
        assert_eq!(item.kind, Some(CompletionItemKind::VARIABLE));
        assert_eq!(item.detail.as_deref(), Some("string"));
        assert_eq!(item.filter_text.as_deref(), Some("$tenant"));

        let Some(tower_lsp::lsp_types::CompletionTextEdit::Edit(edit)) = item.text_edit else {
            panic!("every item must carry an explicit edit range");
        };
        // The edit must cover the whole `$tenant` token, `$` included.
        assert_eq!(edit.range.start.character, 14);
        assert_eq!(edit.range.end.character, 21);
        assert_eq!(edit.new_text, "$tenant");
    }

    #[test]
    fn every_candidate_class_maps_to_a_distinct_icon() {
        let classes = [
            CandidateKind::Field,
            CandidateKind::Table,
            CandidateKind::Param,
            CandidateKind::Function,
            CandidateKind::Method,
            CandidateKind::Namespace,
            CandidateKind::ObjectKey,
            CandidateKind::Index,
        ];
        let mapped: Vec<CompletionItemKind> = classes.into_iter().map(item_kind).collect();
        let unique: std::collections::BTreeSet<i32> = mapped
            .iter()
            .map(|kind| serde_json::to_value(kind).expect("serializable").as_i64().unwrap_or(0) as i32)
            .collect();
        assert_eq!(unique.len(), classes.len(), "icons must not collide");
    }
}
