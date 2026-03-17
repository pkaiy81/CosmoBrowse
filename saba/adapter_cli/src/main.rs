use cosmo_core::nebula_renderer::css::cssom::CssParser;
use cosmo_core::nebula_renderer::css::token::CssTokenizer;
use cosmo_core::nebula_renderer::dom::api::{get_js_content, get_style_content};
use cosmo_core::nebula_renderer::html::parser::HtmlParser;
use cosmo_core::nebula_renderer::html::token::HtmlTokenizer;
use cosmo_core::nebula_renderer::js::ast::JsParser;
use cosmo_core::nebula_renderer::js::runtime::JsRuntime;
use cosmo_core::nebula_renderer::js::token::JsLexer;
use cosmo_core::nebula_renderer::layout::layout_view::LayoutView;
use cosmo_runtime::{AppService, PageViewModel, StarshipApp};
use std::env;
use std::fs;
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
        Ok(Command::VerifyEventLoop {
            fixture_path,
            click_target_id,
        }) => verify_event_loop(&fixture_path, click_target_id.as_deref()),
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
    VerifyEventLoop {
        fixture_path: String,
        click_target_id: Option<String>,
    },
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
        Some("verify-event-loop") => Ok(Command::VerifyEventLoop {
            fixture_path: required_arg(args, 2, "fixture-path")?,
            click_target_id: optional_arg(args, 3),
        }),
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
    let mut app = StarshipApp::default();
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
    let mut app = StarshipApp::default();
    if let Err(error) = app.open_url(url) {
        eprintln!("open_url failed [{}]: {}", error.code, error.message);
        return Err(());
    }

    let view = app.get_page_view();
    print_page_snapshot(&view);
    Ok(())
}

fn activate_link(url: &str, frame_id: &str, href: &str, target: Option<&str>) -> Result<(), ()> {
    let mut app = StarshipApp::default();
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
    let mut app = StarshipApp::default();
    if let Err(error) = app.open_url(url) {
        eprintln!("open_url failed [{}]: {}", error.code, error.message);
        return Err(());
    }

    let json = serde_json::to_string_pretty(&app.get_metrics()).expect("metrics should serialize");
    println!("{json}");
    Ok(())
}

fn verify_event_loop(fixture_path: &str, click_target_id: Option<&str>) -> Result<(), ()> {
    let html = fs::read_to_string(fixture_path).map_err(|error| {
        eprintln!("failed to read fixture {fixture_path}: {error}");
    })?;

    let window = HtmlParser::new(HtmlTokenizer::new(html)).construct_tree();
    let dom = window.borrow().document();

    let script = get_js_content(dom.clone());
    let mut runtime = JsRuntime::new(dom.clone());
    if !script.trim().is_empty() {
        let lexer = JsLexer::new(script);
        let mut parser = JsParser::new(lexer);
        let program = parser.parse_ast();
        runtime.execute(&program);
    }

    if let Some(target_id) = click_target_id {
        // Spec: input/change/click are dispatched through EventTarget dispatch steps.
        // https://dom.spec.whatwg.org/#concept-event-dispatch
        runtime.dispatch_input(target_id);
        runtime.dispatch_change(target_id);
        runtime.dispatch_click(target_id);
    }

    let style = get_style_content(dom.clone());
    let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
    let layout = LayoutView::new(dom, &cssom, 800);
    let display_items = layout.paint();

    println!("fixture: {fixture_path}");
    println!("click_target_id: {}", click_target_id.unwrap_or("<none>"));
    println!(
        "render_pipeline_invalidated: {}",
        runtime.render_pipeline_invalidated()
    );
    println!("display_items: {}", display_items.len());

    let diagnostics = runtime.unsupported_apis();
    if diagnostics.is_empty() {
        println!("diagnostics: <none>");
    } else {
        for line in diagnostics {
            println!("diagnostic: {line}");
        }
    }

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
    println!("  {program} verify-event-loop <fixture-path> [click-target-id]");
}
