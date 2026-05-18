use clap::Parser;
use std::{
    process::{Command, ExitCode},
    time::Instant,
};

#[derive(Debug, Parser)]
#[command(
    name = "orchestrator",
    version,
    about = "Run coding harness benchmark workflows"
)]
struct Cli {
    #[command(subcommand)]
    command: CommandName,
}

#[derive(Debug, clap::Subcommand)]
enum CommandName {
    /// Run a Docker image once and report its exit status.
    RunImage {
        /// Docker image tag or ID to run.
        image: String,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), String> {
    match cli.command {
        CommandName::RunImage { image } => run_image(&image),
    }
}

fn run_image(image: &str) -> Result<(), String> {
    let started = Instant::now();
    println!("running Docker image: {image}");

    let status = Command::new("docker")
        .arg("run")
        .arg("--rm")
        .arg(image)
        .status()
        .map_err(|error| format!("failed to start docker: {error}"))?;

    let duration = started.elapsed();
    match status.code() {
        Some(0) => {
            println!("container completed successfully in {:.2?}", duration);
            Ok(())
        }
        Some(code) => Err(format!(
            "container exited with status {code} after {:.2?}",
            duration
        )),
        None => Err(format!(
            "container terminated without an exit code after {:.2?}",
            duration
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn clap_command_is_valid() {
        Cli::command().debug_assert();
    }
}
