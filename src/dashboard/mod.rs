pub mod event_log;
pub mod pnl_accounting;
pub mod quant_agent;
pub mod runtime_paths;
pub mod server;

#[cfg(test)]
#[path = "ui_layout_tests.rs"]
mod ui_layout_tests;

#[cfg(test)]
#[path = "quant_agent_tests.rs"]
mod quant_agent_tests;

#[cfg(test)]
#[path = "event_log_tests.rs"]
mod event_log_tests;

#[cfg(test)]
#[path = "pnl_state_tests.rs"]
mod pnl_state_tests;
