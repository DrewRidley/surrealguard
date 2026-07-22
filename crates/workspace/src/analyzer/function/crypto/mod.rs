//! `crypto` function-family analysis dispatch.

use surrealdb_types::Kind;
use surrealguard_syntax::ast;

use crate::analyzer::context::AnalysisContext;

pub mod argon2_compare;
pub mod argon2_generate;
pub mod bcrypt_compare;
pub mod bcrypt_generate;
pub mod blake3;
pub mod joaat;
pub mod md5;
pub mod pbkdf2_compare;
pub mod pbkdf2_generate;
pub mod scrypt_compare;
pub mod scrypt_generate;
pub mod sha1;
pub mod sha256;
pub mod sha512;

pub(crate) fn analyze_crypto_function(
    ctx: &mut AnalysisContext<'_>,
    call: &ast::Call,
    path: &str,
    args: &[Kind],
) -> Kind {
    match path {
        "crypto::argon2::compare" => argon2_compare::analyze_crypto_argon2_compare(ctx, call, args),
        "crypto::argon2::generate" => {
            argon2_generate::analyze_crypto_argon2_generate(ctx, call, args)
        }
        "crypto::bcrypt::compare" => bcrypt_compare::analyze_crypto_bcrypt_compare(ctx, call, args),
        "crypto::bcrypt::generate" => {
            bcrypt_generate::analyze_crypto_bcrypt_generate(ctx, call, args)
        }
        "crypto::blake3" => blake3::analyze_crypto_blake3(ctx, call, args),
        "crypto::joaat" => joaat::analyze_crypto_joaat(ctx, call, args),
        "crypto::md5" => md5::analyze_crypto_md5(ctx, call, args),
        "crypto::pbkdf2::compare" => pbkdf2_compare::analyze_crypto_pbkdf2_compare(ctx, call, args),
        "crypto::pbkdf2::generate" => {
            pbkdf2_generate::analyze_crypto_pbkdf2_generate(ctx, call, args)
        }
        "crypto::scrypt::compare" => scrypt_compare::analyze_crypto_scrypt_compare(ctx, call, args),
        "crypto::scrypt::generate" => {
            scrypt_generate::analyze_crypto_scrypt_generate(ctx, call, args)
        }
        "crypto::sha1" => sha1::analyze_crypto_sha1(ctx, call, args),
        "crypto::sha256" => sha256::analyze_crypto_sha256(ctx, call, args),
        "crypto::sha512" => sha512::analyze_crypto_sha512(ctx, call, args),
        _ => crate::analyzer::function::unknown_function(ctx, call),
    }
}
