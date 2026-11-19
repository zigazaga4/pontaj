//! Shared pieces of the Categoria B export tests: the exported workbook read back with every formula recomputed by the
//! tests themselves, months made up for the tests, the edits the screen makes, and the comparison of the file with the app.
#![allow(dead_code)]

pub mod check;
pub mod edits;
pub mod formula;
pub mod month;
pub mod rng;
pub mod workbook;
