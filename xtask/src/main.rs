//! Build automation for `fastfat`.
//!
//! Runs cargo commands with an explicit Windows toolchain so that the MSVC
//! and GNU builds can be used side by side:
//!
//! ```text
//! cargo xtask <msvc|gnu|all> <build|check|clippy|test|run|doc|clean> [args...]
//! ```
//!
//! Each toolchain also passes `--target <triple>`, so the artifacts land in
//! `target/<triple>/` and the two builds never overwrite each other.

use std::path::Path;
use std::process::{Command, ExitCode};

/// Cargo subcommands that accept `--target` and may be forwarded.
const COMMANDS: [&str; 7] = ["build", "check", "clippy", "test", "run", "doc", "clean"];

/// A Windows toolchain `fastfat` can be built with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Toolchain {
    /// `x86_64-pc-windows-msvc`: needs `link.exe` from the Visual Studio
    /// C++ build tools.
    Msvc,
    /// `x86_64-pc-windows-gnu`: self-contained MinGW linker shipped by rustup.
    Gnu,
}

impl Toolchain {
    /// Parses the target selector given on the command line.
    fn parse_selection(name: &str) -> Option<&'static [Self]> {
        match name {
            "msvc" => Some(&[Self::Msvc]),
            "gnu" => Some(&[Self::Gnu]),
            "all" => Some(&[Self::Gnu, Self::Msvc]),
            _ => None,
        }
    }

    /// Short name used in messages.
    const fn name(self) -> &'static str {
        match self {
            Self::Msvc => "msvc",
            Self::Gnu => "gnu",
        }
    }

    /// Rustup toolchain override passed as the first cargo argument.
    const fn rustup_override(self) -> &'static str {
        match self {
            Self::Msvc => "+stable-x86_64-pc-windows-msvc",
            Self::Gnu => "+stable-x86_64-pc-windows-gnu",
        }
    }

    /// Target triple passed with `--target`.
    const fn triple(self) -> &'static str {
        match self {
            Self::Msvc => "x86_64-pc-windows-msvc",
            Self::Gnu => "x86_64-pc-windows-gnu",
        }
    }

    /// Hint printed when a command fails.
    const fn failure_hint(self) -> &'static str {
        match self {
            Self::Msvc => {
                "hint: the MSVC toolchain needs link.exe; install the \"Desktop development with C++\" \
                 workload of Visual Studio (or the Build Tools), and run \
                 `rustup toolchain install stable-x86_64-pc-windows-msvc` if the toolchain is missing"
            }
            Self::Gnu => {
                "hint: run `rustup toolchain install stable-x86_64-pc-windows-gnu` if the toolchain is missing"
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("xtask: {message}");
            ExitCode::FAILURE
        }
    }
}

/// Parses `args` and runs the requested cargo command for each toolchain.
///
/// # Errors
///
/// Returns a message when the arguments are invalid or a cargo invocation
/// fails.
fn run(args: &[String]) -> Result<(), String> {
    let [selection, command, rest @ ..] = args else {
        print_usage();
        return match args.first().map(String::as_str) {
            Some("help" | "-h" | "--help") => Ok(()),
            _ => Err("missing arguments".to_owned()),
        };
    };

    let toolchains = Toolchain::parse_selection(selection)
        .ok_or_else(|| format!("unknown target `{selection}` (expected msvc, gnu or all)"))?;
    if !COMMANDS.contains(&command.as_str()) {
        return Err(format!(
            "unsupported command `{command}` (expected one of: {})",
            COMMANDS.join(", ")
        ));
    }
    if command == "run" && toolchains.len() > 1 {
        return Err("`run` needs a single target: use msvc or gnu".to_owned());
    }

    toolchains
        .iter()
        .try_for_each(|&toolchain| cargo(toolchain, command, rest))
}

/// Runs `cargo +<toolchain> <command> --target <triple> <rest...>` in the
/// workspace root.
///
/// # Errors
///
/// Returns a message when cargo cannot be started or exits unsuccessfully.
fn cargo(toolchain: Toolchain, command: &str, rest: &[String]) -> Result<(), String> {
    eprintln!(
        "xtask: [{}] cargo {} {command} --target {} {}",
        toolchain.name(),
        toolchain.rustup_override(),
        toolchain.triple(),
        rest.join(" ")
    );

    // `cargo` (not `$CARGO`) goes through the rustup proxy, which honours the
    // `+toolchain` override. `RUSTUP_TOOLCHAIN` is inherited from the cargo
    // that runs xtask and is removed so it cannot interfere.
    let status = Command::new("cargo")
        .arg(toolchain.rustup_override())
        .arg(command)
        .arg("--target")
        .arg(toolchain.triple())
        .args(rest)
        .env_remove("RUSTUP_TOOLCHAIN")
        .current_dir(workspace_root())
        .status()
        .map_err(|err| format!("cannot start cargo: {err}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "[{}] cargo {command} failed ({status})\n{}",
            toolchain.name(),
            toolchain.failure_hint()
        ))
    }
}

/// Returns the workspace root (the parent of the xtask package).
fn workspace_root() -> &'static Path {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap_or(manifest_dir)
}

/// Prints the command line help.
fn print_usage() {
    eprintln!(
        "\
Usage: cargo xtask <TARGET> <COMMAND> [ARGS...]

TARGET:
  msvc   stable-x86_64-pc-windows-msvc  (needs Visual Studio C++ build tools)
  gnu    stable-x86_64-pc-windows-gnu
  all    gnu, then msvc (not allowed with `run`)

COMMAND:
  {}

ARGS are forwarded to cargo, e.g.
  cargo xtask gnu build --release
  cargo xtask all clippy --all-targets
  cargo xtask msvc run --release

Artifacts: target/<triple>/<profile>/fastfat.exe",
        COMMANDS.join(", ")
    );
}
