use std::path::PathBuf;
use std::process::ExitCode;

use review_cli::app::{self, App};
use review_cli::debug_log::log;
use review_cli::installation;
use review_cli::mcp_bridge::{self, BridgeOptions};
use review_cli::{logs, setup, status};

const USAGE: &str = "Usage: co-review [command] [dir] [options]

Commands:
  (none) [dir]       start Co-Review (or reuse a running one) and open dir (default: the current directory)
  mcp [dir]          MCP server on stdio for agent harnesses; bridges to Co-Review's /mcp endpoint with dir
                     (default: cwd) as repository, starting Co-Review on the first tool call
  status [dir]       this Co-Review's version and each agent integration's (flagging any that differ), then
                     the open reviews of dir in a running Co-Review; never starts it
                     (--claude-hook: as Claude Code SessionStart context, or nothing)
  setup <pi|omp>     install the Pi or Oh My Pi package that ships with this Co-Review
  logs               whether Co-Review answers, and its recent debug log (windows, backend, agent bridges),
                     merged by time; for when it hangs or fails (--lines <n>, default 80)
  version            print the version
  help               print this help

Options:
  --port <n>         port of the Co-Review to use or start (default: a running one, else 3000; or $CO_REVIEW_PORT)
  --no-open          start without opening a browser or window
  -v, --version      print the version
  -h, --help         print this help
";

struct Arguments {
    command: String,
    rest: Vec<String>,
    port: Option<u16>,
    lines: usize,
    no_open: bool,
    claude_hook: bool,
}

/// Removes `name` and the value after it, returning the value.
fn take_option(arguments: &mut Vec<String>, name: &str) -> Option<String> {
    let index = arguments.iter().position(|argument| argument == name)?;
    arguments.remove(index);
    (index < arguments.len()).then(|| arguments.remove(index))
}

fn take_flag(arguments: &mut Vec<String>, name: &str) -> bool {
    match arguments.iter().position(|argument| argument == name) {
        Some(index) => {
            arguments.remove(index);
            true
        }
        None => false,
    }
}

fn positive<T: std::str::FromStr + Default + PartialEq>(text: Option<String>) -> Option<T> {
    text.and_then(|text| text.trim().parse::<T>().ok())
        .filter(|value| *value != T::default())
}

fn main() -> ExitCode {
    let mut arguments: Vec<String> = std::env::args().skip(1).collect();
    let first = arguments.first().map(String::as_str);
    if matches!(first, Some("help" | "--help" | "-h"))
        || arguments
            .iter()
            .any(|argument| argument == "--help" || argument == "-h")
    {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if matches!(first, Some("version" | "--version" | "-v")) {
        println!("co-review {}", installation::version());
        return ExitCode::SUCCESS;
    }
    let lines = positive(take_option(&mut arguments, "--lines")).unwrap_or(80);
    let port = positive(
        take_option(&mut arguments, "--port").or_else(|| std::env::var("CO_REVIEW_PORT").ok()),
    );
    let no_open = take_flag(&mut arguments, "--no-open");
    let claude_hook = take_flag(&mut arguments, "--claude-hook");
    if let Some(unknown) = arguments.iter().find(|argument| argument.starts_with('-')) {
        eprint!("co-review: unknown option {unknown}\n\n{USAGE}");
        return ExitCode::from(2);
    }
    let command = match arguments.first().map(String::as_str) {
        Some("mcp" | "status" | "setup" | "logs") => arguments.remove(0),
        _ => "start".to_string(),
    };
    let arguments = Arguments {
        command,
        rest: arguments,
        port,
        lines,
        no_open,
        claude_hook,
    };
    match run(arguments) {
        Ok(code) => ExitCode::from(code),
        Err(error) => {
            log(&format!("{error:#}"));
            ExitCode::FAILURE
        }
    }
}

fn run(arguments: Arguments) -> anyhow::Result<u8> {
    let root = installation::absolute(
        &arguments
            .rest
            .first()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(".")),
    );
    match arguments.command.as_str() {
        "mcp" => mcp_bridge::run(BridgeOptions {
            app: App::locate(),
            port: arguments.port,
            root,
            home: installation::co_review_home(),
        }),
        "status" => {
            status::run(&root, arguments.claude_hook);
            Ok(0)
        }
        "setup" => {
            let code = setup::run(arguments.rest.first().map(String::as_str))?;
            Ok(u8::try_from(code).unwrap_or(1))
        }
        "logs" => {
            logs::run(arguments.lines);
            Ok(0)
        }
        _ => {
            let located = App::locate();
            let base =
                app::ensure_server(located.as_ref(), arguments.port, &root, !arguments.no_open)?;
            let url = format!("{base}/#{}", root.display());
            log(&url);
            // The desktop app opens the repository itself.
            if !arguments.no_open && !located.as_ref().is_some_and(App::is_desktop) {
                app::open_url(&url);
            }
            Ok(0)
        }
    }
}
