use saba_app::{AppService, PageViewModel, SabaApp};
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args = env::args().collect::<Vec<_>>();
    let program = args
        .first()
        .cloned()
        .unwrap_or_else(|| "adapter_cli".to_string());

    let result = match parse_command(&args) {
        Ok(Command::OpenUrl(url)) => open_url(&url),
        Ok(Command::GetSnapshot(url)) => get_snapshot(&url),
        Ok(Command::Metrics(url)) => show_metrics(&url),
        Ok(Command::Help) => {
            print_usage(&program);
            Ok(())
        }
        Err(message) => {
            eprintln!("{message}\n");
            print_usage(&program);
            Err(())
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(()) => ExitCode::FAILURE,
    }
}

enum Command {
    OpenUrl(String),
    GetSnapshot(String),
    Metrics(String),
    Help,
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    match args.get(1).map(String::as_str) {
        Some("open-url") => Ok(Command::OpenUrl(required_arg(args, 2, "url")?)),
        Some("get-snapshot") => Ok(Command::GetSnapshot(required_arg(args, 2, "url")?)),
        Some("metrics") => Ok(Command::Metrics(required_arg(args, 2, "url")?)),
        Some("help") | Some("--help") | Some("-h") | None => Ok(Command::Help),
        Some(command) => Err(format!("Unknown command: {command}")),
    }
}

fn required_arg(args: &[String], index: usize, label: &str) -> Result<String, String> {
    args.get(index)
        .cloned()
        .ok_or_else(|| format!("Missing required argument: {label}"))
}

fn open_url(url: &str) -> Result<(), ()> {
    let mut app = SabaApp::default();
    match app.open_url(url) {
        Ok(view) => {
            print_page_summary(&view);
            Ok(())
        }
        Err(error) => {
            eprintln!("open_url failed [{}]: {}", error.code, error.message);
            Err(())
        }
    }
}

fn get_snapshot(url: &str) -> Result<(), ()> {
    let mut app = SabaApp::default();
    if let Err(error) = app.open_url(url) {
        eprintln!("open_url failed [{}]: {}", error.code, error.message);
        return Err(());
    }

    let view = app.get_page_view();
    let json = serde_json::to_string_pretty(&view).expect("page view should serialize");
    println!("{json}");
    Ok(())
}

fn show_metrics(url: &str) -> Result<(), ()> {
    let mut app = SabaApp::default();
    if let Err(error) = app.open_url(url) {
        eprintln!("open_url failed [{}]: {}", error.code, error.message);
        return Err(());
    }

    let json = serde_json::to_string_pretty(&app.get_metrics()).expect("metrics should serialize");
    println!("{json}");
    Ok(())
}

fn print_page_summary(view: &PageViewModel) {
    println!("Title: {}", view.title);
    println!("URL: {}", view.current_url);
    println!("Scene items: {}", view.scene_items.len());
    println!(
        "Content size: width={}, height={}",
        view.content_size.width, view.content_size.height
    );

    if !view.diagnostics.is_empty() {
        println!("Diagnostics: {}", view.diagnostics.join(" | "));
    }
}

fn print_usage(program: &str) {
    println!("Usage:");
    println!("  {program} open-url <url>");
    println!("  {program} get-snapshot <url>");
    println!("  {program} metrics <url>");
}
