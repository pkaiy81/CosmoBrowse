use saba_app::{AppService, RenderSnapshot, SabaApp};
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
        Ok(snapshot) => {
            print_snapshot_summary(&snapshot);
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

    let snapshot = app.get_render_snapshot();
    let json = serde_json::to_string_pretty(&snapshot).expect("snapshot should serialize");
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

fn print_snapshot_summary(snapshot: &RenderSnapshot) {
    println!("Title: {}", snapshot.title);
    println!("URL: {}", snapshot.current_url);
    println!("Text blocks: {}", snapshot.text_blocks.len());
    println!("Links: {}", snapshot.links.len());
    println!(
        "Layout: text_items={}, block_items={}, width={}, height={}",
        snapshot.layout.text_item_count,
        snapshot.layout.block_item_count,
        snapshot.layout.content_width,
        snapshot.layout.content_height
    );

    if !snapshot.diagnostics.is_empty() {
        println!("Diagnostics: {}", snapshot.diagnostics.join(" | "));
    }
}

fn print_usage(program: &str) {
    println!("Usage:");
    println!("  {program} open-url <url>");
    println!("  {program} get-snapshot <url>");
    println!("  {program} metrics <url>");
}