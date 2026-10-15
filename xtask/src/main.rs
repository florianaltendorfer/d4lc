use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use clap::{Parser, Subcommand};

mod locales;

#[derive(Parser)]
#[command(about = "Run the repository's development gates")]
struct Cli {
    #[command(subcommand)]
    task: Task,
}

#[derive(Subcommand)]
enum Task {
    /// Run verification; the full tier includes desktop gates.
    Verify {
        /// Skip GUI pilots and visual smoke.
        #[arg(long)]
        fast: bool,
    },
    /// Run headless GUI pilots, one process per scenario.
    Pilots {
        /// Number of pilot processes in flight.
        #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..))]
        jobs: u32,
        /// Run only pilot names containing this text.
        #[arg(long)]
        filter: Option<String>,
    },
    /// Run the desktop visual smoke gate.
    VisualSmoke,
    /// Rebuild the UI gettext catalogues from the upstream resx files.
    ImportUpstreamLocales {
        /// Fail instead of writing when a generated file is out of date.
        #[arg(long)]
        check: bool,
    },
}

/// The script a gate task runs; `None` for tasks that run in this process.
fn task_command(task: Task) -> Option<Command> {
    // Reuse the same scripts as CI, rather than maintain a second gate list.
    let mut command = match task {
        Task::Verify { fast } => {
            let mut command = Command::new("bash");
            command.arg("scripts/verify_local.sh");
            if fast {
                command.arg("--fast");
            }
            command
        }
        Task::Pilots { jobs, filter } => {
            let mut command = Command::new("python3");
            command.args(["scripts/run_pilot_suite.py", "--jobs", &jobs.to_string()]);
            if let Some(filter) = filter {
                command.args(["--filter", &filter]);
            }
            command
        }
        Task::VisualSmoke => {
            let mut command = Command::new("bash");
            command.arg("scripts/verify_visual_smoke.sh");
            command
        }
        Task::ImportUpstreamLocales { .. } => return None,
    };
    command.current_dir(repository_root());
    Some(command)
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn main() -> ExitCode {
    let task = Cli::parse().task;
    if let Task::ImportUpstreamLocales { check } = task {
        return match locales::run(&repository_root(), check) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("import-upstream-locales: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let Some(mut command) = task_command(task) else {
        return ExitCode::FAILURE;
    };
    match command.status() {
        Ok(status) => ExitCode::from(
            status
                .code()
                .and_then(|code| u8::try_from(code).ok())
                .unwrap_or(1),
        ),
        Err(error) => {
            eprintln!("failed to run development gate: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn commands_preserve_the_script_contracts() {
        for (argv, program, expected) in [
            (
                vec!["xtask", "verify"],
                "bash",
                vec!["scripts/verify_local.sh"],
            ),
            (
                vec!["xtask", "verify", "--fast"],
                "bash",
                vec!["scripts/verify_local.sh", "--fast"],
            ),
            (
                vec!["xtask", "pilots"],
                "python3",
                vec!["scripts/run_pilot_suite.py", "--jobs", "1"],
            ),
            (
                vec!["xtask", "pilots", "--jobs", "2", "--filter", "toggle"],
                "python3",
                vec![
                    "scripts/run_pilot_suite.py",
                    "--jobs",
                    "2",
                    "--filter",
                    "toggle",
                ],
            ),
            (
                vec!["xtask", "visual-smoke"],
                "bash",
                vec!["scripts/verify_visual_smoke.sh"],
            ),
        ] {
            let command = task_command(Cli::try_parse_from(argv).unwrap().task).unwrap();
            assert_eq!(command.get_program(), program);
            assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
            assert!(command
                .get_current_dir()
                .unwrap()
                .join("Cargo.toml")
                .is_file());
        }
        assert!(Cli::try_parse_from(["xtask", "pilots", "--jobs", "0"]).is_err());
        assert!(Cli::try_parse_from(["xtask", "verify", "--unknown"]).is_err());
    }
}
