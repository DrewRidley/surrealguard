mod create; // CREATE statements
mod delete; // DELETE statements
mod insert; // INSERT statements (separate from CREATE)
mod relate; // RELATE statements (for graph relationships)
mod select; // SELECT statements
mod update; // UPDATE statements
mod upsert; // UPSERT statements

pub use create::analyze_create;
pub use delete::analyze_delete;
pub use insert::analyze_insert;
pub use relate::analyze_relate;
pub use select::analyze_select;
pub use update::analyze_update;
pub use upsert::analyze_upsert;
