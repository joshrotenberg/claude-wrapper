//! Opt-in dangerous operations. Currently: bypass permissions.
//!
//! Running the `claude` CLI with `--permission-mode bypassPermissions`
//! turns off all confirmation prompts for tool use. It's legitimate
//! for some automation -- but it's also the fastest way to turn a bug
//! into a destructive action.
//!
//! This module isolates that capability behind a type you have to
//! explicitly reach for ([`DangerousClient`]), and a runtime env-var
//! gate ([`ALLOW_ENV`]) you have to explicitly set.
//!
//! # Example
//!
//! ```no_run
//! # async fn example() -> claude_wrapper::Result<()> {
//! use claude_wrapper::{Claude, QueryCommand};
//! use claude_wrapper::dangerous::DangerousClient;
//!
//! // At process start:
//! //   export CLAUDE_WRAPPER_ALLOW_DANGEROUS=1
//!
//! let claude = Claude::builder().build()?;
//! let dangerous = DangerousClient::new(claude)?;
//!
//! let output = dangerous
//!     .query_bypass(QueryCommand::new("clean up the build artifacts"))
//!     .await?;
//! println!("{}", output.stdout);
//! # Ok(())
//! # }
//! ```
//!
//! # Why this shape
//!
//! - **Separate type.** `DangerousClient::new` is the only public path
//!   to building a bypassed query. If a reader of calling code sees
//!   `DangerousClient`, the danger is obvious at the call site.
//! - **Runtime env-var gate.** The check happens at construction, so
//!   a caller who forgot to set the env-var gets a typed error rather
//!   than silently running with bypass off (which might surprise them)
//!   or silently running with bypass on (which might destroy things).
//! - **Not a cargo feature.** Feature-gating adds a second layer of
//!   friction (recompile) without making the runtime behaviour any
//!   safer. The env-var matches how Go's `claude-code-go/dangerous`
//!   gates the same operation.
//!
//! # Migrating from [`crate::PermissionMode::BypassPermissions`]
//!
//! The enum variant is kept (marked `#[deprecated]`) so existing
//! callers continue to compile with a warning. New code should go
//! through `DangerousClient`.

use crate::Claude;
use crate::command::{ClaudeCommand, query::QueryCommand};
use crate::error::{Error, Result};
use crate::exec::CommandOutput;
#[allow(deprecated)]
use crate::types::PermissionMode;

/// The env-var that must equal `"1"` at process start for
/// [`DangerousClient::new`] to succeed. Set deliberately -- this is
/// the explicit acknowledgement that bypass mode is OK in this process.
pub const ALLOW_ENV: &str = "CLAUDE_WRAPPER_ALLOW_DANGEROUS";

/// Wrapper that lets callers run bypass-permissions queries against an
/// underlying [`Claude`] client. Construction is gated by the
/// [`ALLOW_ENV`] env-var.
#[derive(Debug, Clone)]
pub struct DangerousClient {
    inner: Claude,
}

impl DangerousClient {
    /// Wrap `claude`, refusing to construct unless the [`ALLOW_ENV`]
    /// env-var equals `"1"`.
    ///
    /// The check is made at each construction rather than memoized so
    /// that a test which flips the env-var mid-process sees the change.
    pub fn new(claude: Claude) -> Result<Self> {
        if !is_allowed() {
            return Err(Error::DangerousNotAllowed { env_var: ALLOW_ENV });
        }
        Ok(Self { inner: claude })
    }

    /// Borrow the underlying [`Claude`] for composition with other
    /// wrapper APIs.
    pub fn claude(&self) -> &Claude {
        &self.inner
    }

    /// Run `cmd` with `--permission-mode bypassPermissions`
    /// unconditionally overridden. Any permission mode the caller
    /// already set on `cmd` is replaced.
    pub async fn query_bypass(&self, cmd: QueryCommand) -> Result<CommandOutput> {
        #[allow(deprecated)]
        let cmd = cmd.permission_mode(PermissionMode::BypassPermissions);
        cmd.execute(&self.inner).await
    }
}

fn is_allowed() -> bool {
    std::env::var(ALLOW_ENV).as_deref() == Ok("1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // The env-var is process-global; serialize the tests that touch
    // it so they don't interleave and flip the answer out from under
    // each other.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_allow_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _g = ENV_LOCK.lock().unwrap();
        let prev = std::env::var(ALLOW_ENV).ok();
        // SAFETY: set_var and remove_var are marked unsafe in recent
        // std because multi-threaded processes can race. We serialize
        // with ENV_LOCK above; cargo test runs tests concurrently,
        // but the lock pins this env-var one caller at a time.
        unsafe {
            match value {
                Some(v) => std::env::set_var(ALLOW_ENV, v),
                None => std::env::remove_var(ALLOW_ENV),
            }
        }
        let out = f();
        unsafe {
            match prev {
                Some(v) => std::env::set_var(ALLOW_ENV, v),
                None => std::env::remove_var(ALLOW_ENV),
            }
        }
        out
    }

    #[test]
    fn new_refuses_without_env() {
        with_allow_env(None, || {
            let claude = Claude::builder().binary("/usr/bin/true").build().unwrap();
            let err = DangerousClient::new(claude).unwrap_err();
            assert!(matches!(
                err,
                Error::DangerousNotAllowed { env_var } if env_var == ALLOW_ENV
            ));
        });
    }

    #[test]
    fn new_refuses_with_wrong_value() {
        with_allow_env(Some("true"), || {
            let claude = Claude::builder().binary("/usr/bin/true").build().unwrap();
            assert!(matches!(
                DangerousClient::new(claude).unwrap_err(),
                Error::DangerousNotAllowed { .. }
            ));
        });
        with_allow_env(Some("yes"), || {
            let claude = Claude::builder().binary("/usr/bin/true").build().unwrap();
            assert!(matches!(
                DangerousClient::new(claude).unwrap_err(),
                Error::DangerousNotAllowed { .. }
            ));
        });
    }

    #[test]
    fn new_accepts_with_allow_env_set_to_one() {
        with_allow_env(Some("1"), || {
            let claude = Claude::builder().binary("/usr/bin/true").build().unwrap();
            let d = DangerousClient::new(claude).unwrap();
            // The wrapper exposes the underlying client for
            // composition. Verify the binary threaded through.
            assert_eq!(d.claude().binary(), std::path::Path::new("/usr/bin/true"));
        });
    }

    #[test]
    fn error_message_names_the_env_var() {
        // User-facing error has to be clear enough that a reader
        // knows which env-var to set without digging.
        let e = Error::DangerousNotAllowed { env_var: ALLOW_ENV };
        let s = e.to_string();
        assert!(s.contains(ALLOW_ENV));
    }
}
