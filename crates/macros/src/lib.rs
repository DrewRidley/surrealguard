use proc_macro::TokenStream;
use quote::quote;
use surrealdb::sql::statements::DefineFunctionStatement;
use syn::{parse_macro_input, LitStr};

#[proc_macro]
pub fn kind(input: TokenStream) -> TokenStream {
    let input_str = parse_macro_input!(input as LitStr);
    let type_str = format!("DEFINE FIELD placeholder ON user TYPE {}", input_str.value());

    quote! {
        {
            let stmt = ::surrealdb::sql::parse(#type_str)
                .expect("Failed to parse type")
                .into_iter()
                .next()
                .expect("Empty statement");

            match stmt {
                ::surrealdb::sql::Statement::Define(::surrealdb::sql::statements::DefineStatement::Field(f)) => f.kind.expect("Field definition must include a type"),
                _ => panic!("Expected field definition"),
            }
        }
    }.into()
}
