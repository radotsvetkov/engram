//! Shared presentation layer: theme, markdown rendering, and formatting
//! helpers used by every TUI view and by the CLI's pretty output.

pub mod format;
pub mod markdown;
pub mod theme;

pub use theme::Theme;
