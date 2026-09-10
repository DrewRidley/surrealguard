//! `crypto` function family: every built-in it dispatches, with its analyzer.

use crate::analyzer::function::BuiltinEntry;

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

/// Every `crypto::` built-in the analyzer resolves, in dispatch order.
pub(crate) static CATALOG: &[BuiltinEntry] = &[
    BuiltinEntry::new(
        "crypto::argon2::compare",
        "Whether a plaintext matches an argon2 hash.",
        argon2_compare::signature,
        argon2_compare::analyze_crypto_argon2_compare,
    ),
    BuiltinEntry::new(
        "crypto::argon2::generate",
        "Hashes a string with argon2.",
        argon2_generate::signature,
        argon2_generate::analyze_crypto_argon2_generate,
    ),
    BuiltinEntry::new(
        "crypto::bcrypt::compare",
        "Whether a plaintext matches a bcrypt hash.",
        bcrypt_compare::signature,
        bcrypt_compare::analyze_crypto_bcrypt_compare,
    ),
    BuiltinEntry::new(
        "crypto::bcrypt::generate",
        "Hashes a string with bcrypt.",
        bcrypt_generate::signature,
        bcrypt_generate::analyze_crypto_bcrypt_generate,
    ),
    BuiltinEntry::new(
        "crypto::blake3",
        "The blake3 hash of a string, hex-encoded.",
        blake3::signature,
        blake3::analyze_crypto_blake3,
    ),
    BuiltinEntry::new(
        "crypto::joaat",
        "The Jenkins one-at-a-time hash of a string.",
        joaat::signature,
        joaat::analyze_crypto_joaat,
    ),
    BuiltinEntry::new(
        "crypto::md5",
        "The MD5 hash of a string, hex-encoded.",
        md5::signature,
        md5::analyze_crypto_md5,
    ),
    BuiltinEntry::new(
        "crypto::pbkdf2::compare",
        "Whether a plaintext matches a PBKDF2 hash.",
        pbkdf2_compare::signature,
        pbkdf2_compare::analyze_crypto_pbkdf2_compare,
    ),
    BuiltinEntry::new(
        "crypto::pbkdf2::generate",
        "Hashes a string with PBKDF2.",
        pbkdf2_generate::signature,
        pbkdf2_generate::analyze_crypto_pbkdf2_generate,
    ),
    BuiltinEntry::new(
        "crypto::scrypt::compare",
        "Whether a plaintext matches a scrypt hash.",
        scrypt_compare::signature,
        scrypt_compare::analyze_crypto_scrypt_compare,
    ),
    BuiltinEntry::new(
        "crypto::scrypt::generate",
        "Hashes a string with scrypt.",
        scrypt_generate::signature,
        scrypt_generate::analyze_crypto_scrypt_generate,
    ),
    BuiltinEntry::new(
        "crypto::sha1",
        "The SHA-1 hash of a string, hex-encoded.",
        sha1::signature,
        sha1::analyze_crypto_sha1,
    ),
    BuiltinEntry::new(
        "crypto::sha256",
        "The SHA-256 hash of a string, hex-encoded.",
        sha256::signature,
        sha256::analyze_crypto_sha256,
    ),
    BuiltinEntry::new(
        "crypto::sha512",
        "The SHA-512 hash of a string, hex-encoded.",
        sha512::signature,
        sha512::analyze_crypto_sha512,
    ),
];
