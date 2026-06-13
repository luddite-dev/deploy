use std::{path::Path, time::Duration};

use tokio_util::sync::CancellationToken;

/// A sensible timeout for quick read-only / inspection commands
/// (listing, logs, stats, inspect, status, etc). These should resolve in
/// at most a few seconds; the timeout exists purely to avoid hanging
/// indefinitely on a stuck docker daemon, network mount, or git process.
pub const QUICK_COMMAND_TIMEOUT: Duration = Duration::from_secs(60);

/// Controls for how a command is executed.
///
/// When either timeout or cancel is set, the child is spawned in its own process group so
/// that, on timeout or cancellation, the entire group (the command and any
/// descendants it spawned) is killed together — not just the direct child.
#[derive(Default, Clone)]
pub struct CommandOptions<'a> {
  /// Run the command at a particular path
  pub path: Option<&'a Path>,
  /// Kill the command (and its process group) if this duration elapses
  /// before it finishes.
  pub timeout: Option<Duration>,
  /// Kill the command (and its process group) when this token is
  /// cancelled, allowing cancellation from elsewhere.
  pub cancel: Option<CancellationToken>,
}

impl<'a> CommandOptions<'a> {
  pub fn path(mut self, path: impl Into<Option<&'a Path>>) -> Self {
    self.path = path.into();
    self
  }
}

impl CommandOptions<'_> {
  pub fn timeout(
    mut self,
    timeout: impl Into<Option<Duration>>,
  ) -> Self {
    self.timeout = timeout.into();
    self
  }

  pub fn cancel(
    mut self,
    cancel: impl Into<Option<CancellationToken>>,
  ) -> Self {
    self.cancel = cancel.into();
    self
  }
}
