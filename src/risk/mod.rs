pub mod order_sync;
pub mod profitability;
pub mod risk_manager;

#[cfg(test)]
#[path = "profitability_tests.rs"]
mod profitability_tests;

#[cfg(test)]
#[path = "fee_defaults_tests.rs"]
mod fee_defaults_tests;
