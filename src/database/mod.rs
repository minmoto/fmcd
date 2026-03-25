pub mod instrumented;

pub use instrumented::{DatabaseStats, InstrumentedDatabase};

#[cfg(test)]
mod tests;
