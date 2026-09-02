// A second dependency that also exports a `Thing`, so a glob import of both
// makes the bare name `Thing` ambiguous.
pub struct Thing;

pub fn other_only() {}
