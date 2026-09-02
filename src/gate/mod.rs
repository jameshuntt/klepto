//! Feature gating as data: the desired layout from `klepto.toml`, the
//! gate plan that reaches it, and the cargo check matrix read back as
//! diagnostics. klepto proposes; the consumer edits and runs cargo.

pub mod oracle;
pub mod plan;
pub mod spec;
