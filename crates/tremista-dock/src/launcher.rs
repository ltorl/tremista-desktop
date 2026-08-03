//! Starting applications from a dock click.

use anyhow::{Context, Result};
use std::process::{Child, Command, Stdio};

/// Spawns apps and reaps them.
///
/// The dock is a long-lived process, so every launched app that exits would
/// otherwise linger as a zombie for the life of the session. We keep the
/// handles and poll them rather than installing a SIGCHLD handler, which would
/// mean either a C dependency or unsafe global state for no real gain.
#[derive(Default)]
pub struct Launcher {
    children: Vec<Child>,
}

impl Launcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// Launch an `Exec=` line. Field codes must already be stripped.
    ///
    /// Run through `sh -c` because `Exec=` is shell-ish -- it can carry quoting
    /// and multiple arguments -- and splitting it correctly ourselves would
    /// mean reimplementing the desktop-entry quoting rules.
    pub fn spawn(&mut self, exec: &str) -> Result<()> {
        let child = Command::new("sh")
            .arg("-c")
            .arg(exec)
            // The app must not inherit the dock's stdio: a chatty program
            // would otherwise spam the session log through us.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("launching {exec:?}"))?;

        self.children.push(child);
        self.reap();
        Ok(())
    }

    /// Drop the handles of apps that have already exited.
    pub fn reap(&mut self) {
        self.children.retain_mut(|child| match child.try_wait() {
            // Still running, or we cannot tell -- keep the handle either way.
            Ok(None) => true,
            Ok(Some(_)) => false,
            Err(e) => {
                log::warn!("waiting on child: {e}");
                false
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawned_processes_are_eventually_reaped() {
        let mut launcher = Launcher::new();
        launcher.spawn("true").unwrap();
        // `true` exits immediately, but not necessarily before spawn returns.
        for _ in 0..100 {
            launcher.reap();
            if launcher.children.is_empty() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("child was never reaped");
    }
}
