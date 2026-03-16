use adapter_native::{IpcRequest, NativeAdapter};
use serde::Serialize;
use std::env;
use std::fs;
use std::io::{self, BufRead};
use std::process::ExitCode;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
struct ErrorEnvelope {
    code: String,
    message: String,
}

fn main() -> ExitCode {
    let args = env::args().collect::<Vec<_>>();
    match parse_mode(&args) {
        Mode::Help => {
            print_usage(&args[0]);
            ExitCode::SUCCESS
        }
        Mode::Once { request_json } => run_once(&request_json),
        Mode::Stdin => run_stdin(),
        Mode::Invalid(message) => {
            eprintln!("{message}\n");
            print_usage(&args[0]);
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Mode {
    Help,
    Once { request_json: String },
    Stdin,
    Invalid(String),
}

fn parse_mode(args: &[String]) -> Mode {
    match args.get(1).map(String::as_str) {
        Some("once") => match args.get(2) {
            Some(json) => Mode::Once {
                request_json: json.to_string(),
            },
            None => Mode::Invalid("Missing JSON request for `once` mode".to_string()),
        },
        Some("stdin") => Mode::Stdin,
        Some("help") | Some("--help") | Some("-h") | None => Mode::Help,
        Some(other) => Mode::Invalid(format!("Unknown mode: {other}")),
    }
}

fn run_once(request_json: &str) -> ExitCode {
    let adapter = NativeAdapter::default();
    let request_source = match load_request_source(request_json) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::FAILURE;
        }
    };

    match parse_request(&request_source) {
        Ok(request) => write_dispatch_result(&adapter, request),
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

fn load_request_source(source_or_path: &str) -> Result<String, String> {
    if let Some(path) = source_or_path.strip_prefix('@') {
        fs::read_to_string(path)
            .map_err(|error| format!("failed to read request file '{path}': {error}"))
    } else {
        Ok(source_or_path.to_string())
    }
}

fn run_stdin() -> ExitCode {
    let adapter = NativeAdapter::default();
    let mut had_error = false;

    for line in io::stdin().lock().lines() {
        let line = match line {
            Ok(value) => value,
            Err(error) => {
                eprintln!("failed to read stdin: {error}");
                return ExitCode::FAILURE;
            }
        };

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match parse_request(trimmed) {
            Ok(request) => {
                if write_dispatch_result(&adapter, request) == ExitCode::FAILURE {
                    had_error = true;
                }
            }
            Err(message) => {
                had_error = true;
                print_json(&ErrorEnvelope {
                    code: "invalid_request".to_string(),
                    message,
                });
            }
        }
    }

    if had_error {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn parse_request(raw: &str) -> Result<IpcRequest, String> {
    serde_json::from_str(raw).map_err(|error| format!("failed to parse request JSON: {error}"))
}

fn write_dispatch_result(adapter: &NativeAdapter, request: IpcRequest) -> ExitCode {
    match adapter.dispatch(request) {
        Ok(response) => {
            print_json(&response);
            ExitCode::SUCCESS
        }
        Err(error) => {
            print_json(&ErrorEnvelope {
                code: error.code,
                message: error.message,
            });
            ExitCode::FAILURE
        }
    }
}

fn print_json<T: Serialize>(value: &T) {
    let payload = serde_json::to_string(value).expect("response should serialize");
    println!("{payload}");
}

fn print_usage(program: &str) {
    println!("Usage:");
    println!("  {program} once '<request-json>'");
    println!("  {program} once @request.json");
    println!("  {program} stdin");
    println!();
    println!("Request JSON example:");
    println!(
        "  {{\"type\":\"open_url\",\"payload\":{{\"url\":\"https://example.com\"}}}}"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_mode_once_uses_payload_argument() {
        let args = vec![
            "native_ipc_cli".to_string(),
            "once".to_string(),
            "{\"type\":\"get_page_view\"}".to_string(),
        ];

        assert_eq!(
            parse_mode(&args),
            Mode::Once {
                request_json: "{\"type\":\"get_page_view\"}".to_string(),
            }
        );
    }

    #[test]
    fn parse_mode_reports_missing_once_argument() {
        let args = vec!["native_ipc_cli".to_string(), "once".to_string()];
        assert_eq!(
            parse_mode(&args),
            Mode::Invalid("Missing JSON request for `once` mode".to_string())
        );
    }

    #[test]
    fn load_request_source_reads_inline_or_file() {
        let inline = "{\"type\":\"list_tabs\"}";
        assert_eq!(load_request_source(inline).unwrap(), inline);

        let temp_path = std::env::temp_dir().join("native_ipc_cli_request_test.json");
        fs::write(&temp_path, inline).expect("should write temp request");
        let loaded = load_request_source(&format!("@{}", temp_path.display())).unwrap();
        assert_eq!(loaded, inline);
        let _ = fs::remove_file(temp_path);
    }

    #[test]
    fn parse_request_accepts_valid_shape() {
        let request = parse_request("{\"type\":\"get_page_view\"}").unwrap();
        assert!(matches!(request, IpcRequest::GetPageView));
    }

    #[test]
    fn parse_request_rejects_invalid_json() {
        let err = parse_request("{not-json}").unwrap_err();
        assert!(err.contains("failed to parse request JSON"));
    }
}
