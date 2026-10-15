//! The `x11-command` capture backend's process boundary: it captures through
//! the external `xwd` program. Only that explicitly selected backend (and the
//! `auto` fallback of the debug CLI) runs it; window discovery and the live
//! capture path are native (`x11_discovery`, `native_x11`).

use std::process::Command;

/// Runs one external program and returns its standard output. Tests inject
/// recorded output through this seam.
pub trait X11CommandRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<Vec<u8>, String>;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct SystemX11CommandRunner;

impl X11CommandRunner for SystemX11CommandRunner {
    fn run(&self, program: &str, args: &[String]) -> Result<Vec<u8>, String> {
        let output = Command::new(program)
            .args(args)
            .output()
            .map_err(|error| format!("failed to execute {program}: {error}"))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let rendered_args = args.join(" ");
            Err(format!(
                "{program} {rendered_args} exited with status {}{}",
                output.status,
                if stderr.is_empty() {
                    String::new()
                } else {
                    format!(": {stderr}")
                }
            ))
        }
    }
}
