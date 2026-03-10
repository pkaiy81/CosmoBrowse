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
        Ok(Command::ActivateLink {
            url,
            frame_id,
            href,
            target,
        }) => activate_link(&url, &frame_id, &href, target.as_deref()),
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
    ActivateLink {
        url: String,
        frame_id: String,
        href: String,
        target: Option<String>,
    },
    Metrics(String),
    Help,
}

fn parse_command(args: &[String]) -> Result<Command, String> {
    match args.get(1).map(String::as_str) {
        Some("open-url") => Ok(Command::OpenUrl(required_arg(args, 2, "url")?)),
        Some("get-snapshot") => Ok(Command::GetSnapshot(required_arg(args, 2, "url")?)),
        Some("activate-link") => Ok(Command::ActivateLink {
            url: required_arg(args, 2, "url")?,
            frame_id: required_arg(args, 3, "frame-id")?,
            href: required_arg(args, 4, "href")?,
            target: optional_arg(args, 5),
        }),
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

fn optional_arg(args: &[String], index: usize) -> Option<String> {
    args.get(index)
        .cloned()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
    print_page_snapshot(&view);
    Ok(())
}

fn activate_link(url: &str, frame_id: &str, href: &str, target: Option<&str>) -> Result<(), ()> {
    let mut app = SabaApp::default();
    if let Err(error) = app.open_url(url) {
        eprintln!("open_url failed [{}]: {}", error.code, error.message);
        return Err(());
    }

    match app.activate_link(frame_id, href, target) {
        Ok(view) => {
            print_page_snapshot(&view);
            Ok(())
        }
        Err(error) => {
            eprintln!("activate_link failed [{}]: {}", error.code, error.message);
            Err(())
        }
    }
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

fn print_page_snapshot(view: &PageViewModel) {
    let json = serde_json::to_string_pretty(view).expect("page view should serialize");
    println!("{json}");
}

fn print_usage(program: &str) {
    println!("Usage:");
    println!("  {program} open-url <url>");
    println!("  {program} get-snapshot <url>");
    println!("  {program} activate-link <url> <frame-id> <href> [target]");
    println!("  {program} metrics <url>");
}
