use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, LitStr};

#[proc_macro]
pub fn kind(input: TokenStream) -> TokenStream {
    // Parse the input literal.
    let input_str = parse_macro_input!(input as LitStr);
    let type_str = format!(
        "DEFINE FIELD placeholder ON user TYPE {}",
        input_str.value()
    );

    // Attempt to parse at macro expansion time.
    let parsed = ::surrealdb::sql::parse(&type_str)
        .map_err(|err| err.to_string())
        .and_then(|mut stmts| {
            stmts
                .pop()
                .ok_or_else(|| "Empty statement generated".to_string())
        });

    if let Err(err_msg) = parsed {
        // Emit a compile_error! invocation.
        return quote! {
            compile_error!(#err_msg)
        }
        .into();
    }

    // Otherwise, generate code that parses the type string at runtime.
    quote! {
        {
            let stmt = ::surrealdb::sql::parse(#type_str)
                .expect("Failed to parse type")
                .into_iter()
                .next()
                .expect("Empty statement");

            match stmt {
                ::surrealdb::sql::Statement::Define(::surrealdb::sql::statements::DefineStatement::Field(f)) => {
                    f.kind.expect("Field definition must include a type")
                },
                _ => panic!("Expected field definition"),
            }
        }
    }
    .into()
}
