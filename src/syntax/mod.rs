//! What is written: the syn walk that collects items, imports, calls,
//! attributes and use sites, plus the queries, index, snapshot and report
//! over that raw material.

pub mod extract;
pub mod find;
pub mod imports_ext;
pub mod index;
pub mod query;
pub mod report;
pub mod snapshot;
pub mod use_sites;
