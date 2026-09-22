//! Command-line surface for `portez`.

use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;

use thiserror::Error;
use usage::{Args, Cli as UsageCli, RunWith, Subcommands, ValueEnum};

use crate::registry::{self, DEFAULT_NAME, Registry, RegistryError};

/// Stable development ports, one per directory and name.
///
/// `portez [NAME]` prints the port registered for NAME in the current
/// directory, allocating the lowest free one on first use. Stdout carries
/// only the number so `PORT=$(portez)` works in scripts.
#[derive(UsageCli, Debug)]
#[usage(
    bin = "portez",
    version = env!("CARGO_PKG_VERSION"),
    completion,
    unknown_flags = "error",
    args_override_self = false,
    default_subcommand = "get"
)]
pub struct Cli {
    /// Project directory; defaults to the current directory.
    #[usage(short = 'C', long, global, value_hint = usage::ValueHint::DirPath)]
    dir: Option<PathBuf>,
    /// Registry file; defaults to `$PORTEZ_CONFIG` or `~/.config/portez/ports.toml`.
    #[usage(long, global, value_hint = usage::ValueHint::FilePath, extensions("toml"))]
    registry: Option<PathBuf>,
    /// Suppress the first-use notice on stderr.
    #[usage(short = 'q', long, global)]
    quiet: bool,
    #[usage(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommands, Debug)]
#[usage(run_with)]
enum Command {
    Get(Get),
    List(List),
    Rm(Rm),
    Path(PathCmd),
    Completion(Completion),
}

/// Print the port for NAME here, registering one on first use.
#[derive(Args, Debug)]
#[usage(effect = "write")]
struct Get {
    /// Port name within the directory; defaults to `main`.
    name: Option<String>,
    /// Fail instead of registering when NAME has no port yet.
    #[usage(long)]
    existing: bool,
}

/// List every registered port.
#[derive(Args, Debug)]
#[usage(effect = "read", visible_alias = "ls")]
struct List {
    /// Emit a JSON array instead of a table.
    #[usage(long)]
    json: bool,
}

/// Forget the port registered for NAME in this directory.
#[derive(Args, Debug)]
#[usage(effect = "write", visible_alias = "remove")]
struct Rm {
    /// Port name to forget; defaults to `main`.
    name: Option<String>,
    /// Forget every name registered for the directory.
    #[usage(long, conflicts = "name")]
    all: bool,
}

/// Print the registry file path.
#[derive(Args, Debug)]
#[usage(name = "path", effect = "read")]
struct PathCmd;

/// Print a shell completion script.
#[derive(Args, Debug)]
#[usage(effect = "read")]
struct Completion {
    #[usage(value_enum)]
    shell: Shell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
#[allow(clippy::enum_variant_names)]
enum Shell {
    Bash,
    Zsh,
    Fish,
    Elvish,
    #[usage(name = "nushell", aliases = ["nu"])]
    Nushell,
    #[usage(name = "powershell", aliases = ["pwsh"])]
    PowerShell,
}

impl From<Shell> for usage::complete::Shell {
    fn from(value: Shell) -> Self {
        match value {
            Shell::Bash => Self::Bash,
            Shell::Zsh => Self::Zsh,
            Shell::Fish => Self::Fish,
            Shell::Elvish => Self::Elvish,
            Shell::Nushell => Self::Nu,
            Shell::PowerShell => Self::PowerShell,
        }
    }
}

#[derive(Debug, Error)]
enum AppError {
    #[error(transparent)]
    Registry(#[from] RegistryError),
    #[error("cannot resolve directory {}: {source}", dir.display())]
    Dir { dir: PathBuf, source: io::Error },
    #[error("cannot write output: {0}")]
    Io(#[from] io::Error),
    #[error("cannot encode JSON: {0}")]
    Json(#[from] serde_json::Error),
}

type CommandResult = Result<(), AppError>;

struct Context {
    registry: PathBuf,
    dir: Option<PathBuf>,
    quiet: bool,
}

impl Context {
    fn new(cli: &Cli) -> Result<Self, AppError> {
        let registry = match &cli.registry {
            Some(path) => path.clone(),
            None => Registry::default_path()?,
        };
        Ok(Self {
            registry,
            dir: cli.dir.clone(),
            quiet: cli.quiet,
        })
    }

    /// Canonical key for the project directory.
    fn dir_key(&self) -> Result<String, AppError> {
        let dir = match &self.dir {
            Some(dir) => dir.clone(),
            None => std::env::current_dir().map_err(|source| AppError::Dir {
                dir: PathBuf::from("."),
                source,
            })?,
        };
        registry::canonical_dir(&dir).map_err(|source| AppError::Dir { dir, source })
    }
}

impl RunWith<&Context> for Get {
    type Output = CommandResult;

    fn run_with(self, context: &Context) -> Self::Output {
        let dir = context.dir_key()?;
        let name = self.name.as_deref().unwrap_or(DEFAULT_NAME);
        let (port, created) = if self.existing {
            let registry = Registry::load(&context.registry)?;
            let port = registry
                .lookup(&dir, name)
                .ok_or_else(|| RegistryError::NotRegistered {
                    dir: dir.clone(),
                    name: name.to_owned(),
                })?;
            (port, false)
        } else {
            Registry::edit(&context.registry, |registry| registry.assign(&dir, name))?
        };
        if created && !context.quiet {
            eprintln!("portez: registered {name} → {port} for {dir}");
        }
        line(port)
    }
}

impl RunWith<&Context> for List {
    type Output = CommandResult;

    fn run_with(self, context: &Context) -> Self::Output {
        let registry = Registry::load(&context.registry)?;
        let rows = registry.assignments();
        if self.json {
            let stdout = io::stdout();
            let mut lock = stdout.lock();
            serde_json::to_writer_pretty(&mut lock, &rows)?;
            writeln!(lock)?;
            return Ok(());
        }
        if rows.is_empty() {
            if !context.quiet && io::stderr().is_terminal() {
                eprintln!(
                    "portez: nothing registered in {}",
                    registry.path().display()
                );
            }
            return Ok(());
        }
        let name_width = rows.iter().map(|r| r.name.len()).max().unwrap_or(4).max(4);
        let stdout = io::stdout();
        let mut out = stdout.lock();
        writeln!(out, "{:<5}  {:<name_width$}  DIR", "PORT", "NAME")?;
        for row in rows {
            writeln!(
                out,
                "{:<5}  {:<name_width$}  {}",
                row.port, row.name, row.dir
            )?;
        }
        Ok(())
    }
}

impl RunWith<&Context> for Rm {
    type Output = CommandResult;

    fn run_with(self, context: &Context) -> Self::Output {
        let dir = context.dir_key()?;
        let name = if self.all {
            None
        } else {
            Some(self.name.as_deref().unwrap_or(DEFAULT_NAME))
        };
        let removed = Registry::edit(&context.registry, |registry| registry.remove(&dir, name))?;
        if !context.quiet {
            for row in removed {
                eprintln!(
                    "portez: removed {} → {} for {}",
                    row.name, row.port, row.dir
                );
            }
        }
        Ok(())
    }
}

impl RunWith<&Context> for PathCmd {
    type Output = CommandResult;

    fn run_with(self, context: &Context) -> Self::Output {
        line(context.registry.display())
    }
}

impl RunWith<&Context> for Completion {
    type Output = CommandResult;

    fn run_with(self, _context: &Context) -> Self::Output {
        print!("{}", Cli::completion_script(self.shell.into()));
        Ok(())
    }
}

fn line(value: impl std::fmt::Display) -> CommandResult {
    let stdout = io::stdout();
    let mut lock = stdout.lock();
    writeln!(lock, "{value}")?;
    Ok(())
}

/// Parse `args` (without the program name), run the command, and return the
/// process exit code: 0 on success, 1 on an application error, 2 on a usage
/// error.
pub fn entry(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = args.into_iter().collect::<Vec<_>>();
    let refs = args.iter().map(OsString::as_os_str).collect::<Vec<_>>();

    if let Some(spec) = Cli::spec_request(&refs) {
        print!("{spec}");
        return 0;
    }
    if let Some(completion) = Cli::completion_request(&args) {
        print!("{completion}");
        return 0;
    }

    let Ok(cli) = Cli::parse_from(&refs) else {
        let usage::embedded::Outcome::Exit(exit) = Cli::embedded_outcome(&args) else {
            unreachable!("an argv that failed parse_from cannot parse through embedded_outcome")
        };
        if exit.stderr {
            eprint!("{}", exit.text);
        } else {
            print!("{}", exit.text);
        }
        return exit.code;
    };

    let context = match Context::new(&cli) {
        Ok(context) => context,
        Err(error) => return report(&error),
    };
    // With no arguments at all the default subcommand is not applied, so a
    // bare `portez` behaves like `portez get`.
    let command = cli.command.unwrap_or(Command::Get(Get {
        name: None,
        existing: false,
    }));
    match command.run_with(&context) {
        Ok(()) => 0,
        Err(error) => report(&error),
    }
}

fn report(error: &AppError) -> i32 {
    eprintln!("error: {error}");
    1
}
