pub mod agents;
pub mod auth;
pub mod doctor;
pub mod marketplace;
pub mod mcp;
pub mod plugin;
pub mod query;
pub mod raw;
pub mod version;

use std::future::Future;

use crate::Claude;
use crate::error::Result;

/// Trait implemented by all claude CLI command builders.
///
/// Each command defines its own `Output` type and builds its argument
/// list via `args()`. Execution is dispatched through the shared `Claude`
/// client which provides binary path, environment, and timeout config.
pub trait ClaudeCommand: Send + Sync {
    /// The typed result of executing this command.
    type Output: Send;

    /// Build the CLI argument list for this command.
    fn args(&self) -> Vec<String>;

    /// Execute the command using the given claude client.
    fn execute(&self, claude: &Claude) -> impl Future<Output = Result<Self::Output>> + Send;
}
