pub mod agents;
pub mod auth;
pub mod auto_mode;
pub mod doctor;
pub mod install;
pub mod marketplace;
pub mod mcp;
pub mod plugin;
pub mod project;
pub mod query;
pub mod raw;
pub mod update;
pub mod version;

#[cfg(feature = "async")]
use std::future::Future;

use crate::Claude;
use crate::error::Result;

/// Trait implemented by all claude CLI command builders.
///
/// Each command defines its own `Output` type and builds its argument
/// list via `args()`. Execution is dispatched through the shared `Claude`
/// client which provides binary path, environment, and timeout config.
///
/// The async `execute` method is only present when the `async` feature
/// is enabled. In sync-only builds, callers reach the blocking path
/// via [`ClaudeCommandSyncExt::execute_sync`].
pub trait ClaudeCommand: Send + Sync {
    /// The typed result of executing this command.
    type Output: Send;

    /// Build the CLI argument list for this command.
    fn args(&self) -> Vec<String>;

    /// Execute the command using the given claude client.
    #[cfg(feature = "async")]
    fn execute(&self, claude: &Claude) -> impl Future<Output = Result<Self::Output>> + Send;
}

/// Blocking `execute_sync` for any command that returns `CommandOutput`.
///
/// Most command builders (all except the json-decoding convenience
/// methods) produce `CommandOutput` — this extension trait gives them
/// a one-line blocking entry point that routes through
/// [`crate::exec::run_claude_sync`].
///
/// ```no_run
/// # #[cfg(feature = "sync")]
/// # {
/// use claude_wrapper::{Claude, ClaudeCommandSyncExt, VersionCommand};
///
/// # fn example() -> claude_wrapper::Result<()> {
/// let claude = Claude::builder().build()?;
/// let out = VersionCommand::new().execute_sync(&claude)?;
/// println!("{}", out.stdout);
/// # Ok(())
/// # }
/// # }
/// ```
///
/// Commands with custom execute paths (e.g. [`crate::QueryCommand`],
/// which honours `retry_policy`) override this via an inherent method
/// of the same name — inherent-method resolution wins, so callers
/// don't need to disambiguate.
#[cfg(feature = "sync")]
pub trait ClaudeCommandSyncExt {
    /// Blocking analog of [`ClaudeCommand::execute`] for commands
    /// producing `CommandOutput`.
    fn execute_sync(&self, claude: &Claude) -> Result<crate::exec::CommandOutput>;
}

#[cfg(feature = "sync")]
impl<T> ClaudeCommandSyncExt for T
where
    T: ClaudeCommand<Output = crate::exec::CommandOutput>,
{
    fn execute_sync(&self, claude: &Claude) -> Result<crate::exec::CommandOutput> {
        crate::exec::run_claude_sync(claude, self.args())
    }
}
