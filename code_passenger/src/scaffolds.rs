// src/scaffolds.rs
use crate::error::{PassengerError, Result};
use crate::model::ScaffoldKind;

pub trait ScaffoldPlan: Send + Sync {
    fn id(&self) -> &'static str;

    /// Template selection by “kind”.
    /// Templates can use placeholders:
    /// - {snake}  => snake_case name
    /// - {Ty}     => PascalCase type name
    fn template(&self, kind: ScaffoldKind) -> &'static str;
}

#[derive(Debug, Default)]
pub struct DefaultScaffoldPlan;

impl ScaffoldPlan for DefaultScaffoldPlan {
    fn id(&self) -> &'static str { "default" }

    fn template(&self, kind: ScaffoldKind) -> &'static str {
        match kind {
            ScaffoldKind::Module => r#"//! {snake}.rs
//! code_passenger scaffold (module)

pub mod {snake} {
    // TODO: implement
}
"#,

            ScaffoldKind::Component => r#"//! {snake}.rs
//! code_passenger scaffold (component)

#[derive(Debug, Default)]
pub struct {Ty};

impl {Ty} {
    pub fn new() -> Self { Self::default() }
}
"#,

            ScaffoldKind::Service => r#"//! {snake}.rs
//! code_passenger scaffold (service)

#[derive(Debug, Default)]
pub struct {Ty};

impl {Ty} {
    pub fn run(&self) {
        // TODO: service logic
    }
}
"#,

            ScaffoldKind::Test => r#"//! {snake}.rs
//! code_passenger scaffold (test)

#[test]
fn {snake}_smoke() {
    assert!(true);
}
"#,
        }
    }
}

pub fn get_scaffold_plan(id: &str) -> Result<Box<dyn ScaffoldPlan>> {
    match id {
        "default" => Ok(Box::new(DefaultScaffoldPlan)),
        _ => Err(PassengerError::Unsupported(format!("scaffold plan '{id}' not available"))),
    }
}