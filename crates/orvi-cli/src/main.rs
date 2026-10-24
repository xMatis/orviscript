use std::env;
use std::ffi::OsString;
use std::process::ExitCode;

const HELP: &str = "OrviScript

Usage: orviscript [OPTIONS]

Options:
  -h, --help     Print help
  -V, --version  Print version";

fn command_output(args: &[OsString]) -> Result<String, String> {
    match args {
        [] => Ok(HELP.to_owned()),
        [arg] if arg == "-h" || arg == "--help" => Ok(HELP.to_owned()),
        [arg] if arg == "-V" || arg == "--version" => {
            Ok(format!("orviscript {}", env!("CARGO_PKG_VERSION")))
        }
        [arg] => Err(format!("unexpected argument '{}'", arg.to_string_lossy())),
        _ => Err("expected at most one argument".to_owned()),
    }
}

fn main() -> ExitCode {
    let args = env::args_os().skip(1).collect::<Vec<_>>();

    match command_output(&args) {
        Ok(output) => {
            println!("{output}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("error: {message}\n\nFor usage information, try 'orviscript --help'.");
            ExitCode::from(2)
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{HELP, command_output};
    use std::ffi::OsString;

    #[test]
    fn displays_help_without_arguments() {
        assert_eq!(command_output(&[]), Ok(HELP.to_owned()));
    }

    #[test]
    fn displays_help_for_long_option() {
        assert_eq!(
            command_output(&[OsString::from("--help")]),
            Ok(HELP.to_owned())
        );
    }

    #[test]
    fn displays_package_version() {
        assert_eq!(
            command_output(&[OsString::from("--version")]),
            Ok(format!("orviscript {}", env!("CARGO_PKG_VERSION")))
        );
    }

    #[test]
    fn rejects_unknown_arguments() {
        assert_eq!(
            command_output(&[OsString::from("run")]),
            Err("unexpected argument 'run'".to_owned())
        );
    }
}
