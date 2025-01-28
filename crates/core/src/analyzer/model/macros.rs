
// model/macros.rs
#[macro_export]
macro_rules! type_ {
    // Basic types
    (null) => { Type::null() };
    (bool) => { Type::bool() };
    (int) => { Type::int() };
    (float) => { Type::float() };
    (string) => { Type::string() };

    // Array type
    (array[$inner:expr]) => {
        Type::array($inner)
    };

    // Record type
    (record $table:expr) => {
        Type::record($table.to_string())
    };

    // Object type with fields
    (object { $($field:ident: $type:expr),* }) => {{
        let mut fields = HashMap::new();
        $(
            fields.insert(stringify!($field).to_string(), $type);
        )*
        Type::object(fields)
    }};

    // Type with metadata
    ($kind:expr, {
        $(permissions: $perms:expr,)?
        $(default: $default:expr,)?
        $(assert: $assert:expr,)?
    }) => {{
        let mut t = $kind;
        $(t = t.with_permissions($perms);)?
        $(t = t.with_default($default);)?
        $(t = t.with_assert($assert);)?
        t
    }};
}
