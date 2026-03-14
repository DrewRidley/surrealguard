//! Signature help — shows function signatures when the user types `(` or `,`.

use tower_lsp::lsp_types::{
    ParameterInformation, ParameterLabel, Position, SignatureHelp, SignatureInformation,
};

use surrealguard_analyzer::Context;
use surrealguard_analyzer::types::display_kind;

use crate::hover::BUILTIN_FUNCTIONS;
use crate::text::position_to_offset;

/// Resolve signature help for a document at a given cursor position.
///
/// Scans backwards from the cursor to find the function name before `(`,
/// then looks up the signature in builtins or custom functions.
pub fn resolve(source: &str, position: Position, ctx: &Context) -> Option<SignatureHelp> {
    let offset = position_to_offset(source, position);
    let before = &source[..offset.min(source.len())];

    // Find the matching `(` for the current argument list, counting nesting.
    let (fn_name, active_param) = find_function_context(before)?;

    // Try builtin functions first.
    if let Some(sig) = builtin_signature(&fn_name, active_param) {
        return Some(sig);
    }

    // Try custom functions (fn::name).
    if let Some(sig) = custom_function_signature(&fn_name, ctx, active_param) {
        return Some(sig);
    }

    None
}

/// Scan backwards from cursor to find the enclosing function call.
/// Returns (function_name, active_parameter_index).
fn find_function_context(text: &str) -> Option<(String, u32)> {
    let bytes = text.as_bytes();
    let mut depth = 0i32;
    let mut comma_count = 0u32;

    // Walk backwards to find the opening `(` at depth 0.
    let mut i = bytes.len();
    while i > 0 {
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' => {
                if depth == 0 {
                    // Found the opening paren — extract the function name before it.
                    let before_paren = &text[..i];
                    let fn_name = extract_function_name(before_paren)?;
                    return Some((fn_name, comma_count));
                }
                depth -= 1;
            }
            b',' if depth == 0 => comma_count += 1,
            // Skip string literals to avoid counting commas/parens inside them.
            b'\'' | b'"' => {
                let quote = bytes[i];
                while i > 0 {
                    i -= 1;
                    if bytes[i] == quote && (i == 0 || bytes[i - 1] != b'\\') {
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    None
}

/// Extract the function name immediately before `(`.
/// Handles names like `string::len`, `fn::my_func`, `count`.
fn extract_function_name(text: &str) -> Option<String> {
    let trimmed = text.trim_end();
    if trimmed.is_empty() {
        return None;
    }

    // Walk backwards to collect valid function name characters (alphanumeric, _, ::).
    let bytes = trimmed.as_bytes();
    let mut end = bytes.len();
    let mut start = end;

    while start > 0 {
        let b = bytes[start - 1];
        if b.is_ascii_alphanumeric() || b == b'_' {
            start -= 1;
        } else if b == b':' && start >= 2 && bytes[start - 2] == b':' {
            start -= 2;
        } else {
            break;
        }
    }

    if start == end {
        return None;
    }

    let name = &trimmed[start..end];
    if name.is_empty() {
        return None;
    }

    Some(name.to_string())
}

/// Look up a builtin function and return SignatureHelp.
fn builtin_signature(name: &str, active_param: u32) -> Option<SignatureHelp> {
    let func = BUILTIN_FUNCTIONS.iter().find(|f| f.name == name)?;
    let params = parse_signature_params(func.signature);

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label: func.signature.to_string(),
            documentation: Some(tower_lsp::lsp_types::Documentation::String(
                func.summary.to_string(),
            )),
            parameters: Some(params),
            active_parameter: Some(active_param),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param),
    })
}

/// Look up a custom function (fn::name) and return SignatureHelp.
fn custom_function_signature(
    name: &str,
    ctx: &Context,
    active_param: u32,
) -> Option<SignatureHelp> {
    // Custom functions are stored with just their name (e.g., "my_func"),
    // but may be called as "fn::my_func".
    let lookup = if let Some(stripped) = name.strip_prefix("fn::") {
        stripped
    } else {
        name
    };

    let func_def = ctx.get_function(lookup)?;

    // Build signature label: fn::name($param1: type, $param2: type) -> return_type
    let param_strs: Vec<String> = func_def
        .params
        .iter()
        .map(|(pname, ptype)| format!("{}: {}", pname, display_kind(ptype)))
        .collect();

    let mut label = format!("fn::{}({})", func_def.name, param_strs.join(", "));
    if let Some(ref ret) = func_def.return_type {
        label.push_str(&format!(" -> {}", display_kind(ret)));
    }

    let parameters: Vec<ParameterInformation> = func_def
        .params
        .iter()
        .map(|(pname, ptype)| {
            let param_label = format!("{}: {}", pname, display_kind(ptype));
            ParameterInformation {
                label: ParameterLabel::Simple(param_label),
                documentation: None,
            }
        })
        .collect();

    Some(SignatureHelp {
        signatures: vec![SignatureInformation {
            label,
            documentation: None,
            parameters: Some(parameters),
            active_parameter: Some(active_param),
        }],
        active_signature: Some(0),
        active_parameter: Some(active_param),
    })
}

/// Parse parameter info from a signature string like
/// `string::contains(value: string, search: string) -> bool`.
fn parse_signature_params(signature: &str) -> Vec<ParameterInformation> {
    let open = match signature.find('(') {
        Some(i) => i,
        None => return vec![],
    };
    let close = match signature.rfind(')') {
        Some(i) => i,
        None => return vec![],
    };

    let params_str = &signature[open + 1..close];
    if params_str.trim().is_empty() {
        return vec![];
    }

    params_str
        .split(',')
        .map(|p| {
            let trimmed = p.trim().to_string();
            ParameterInformation {
                label: ParameterLabel::Simple(trimmed),
                documentation: None,
            }
        })
        .collect()
}
