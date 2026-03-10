use serde::Serialize;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl AppError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self { code: "validation_error".to_string(), message: message.into(), retryable: false }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self { code: "network_error".to_string(), message: message.into(), retryable: true }
    }

    pub fn tls(message: impl Into<String>) -> Self {
        Self { code: "tls_error".to_string(), message: message.into(), retryable: true }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self { code: "parse_error".to_string(), message: message.into(), retryable: false }
    }

    pub fn state(message: impl Into<String>) -> Self {
        Self { code: "invalid_state".to_string(), message: message.into(), retryable: false }
    }
}

impl core::fmt::Display for AppError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContentSize {
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct FrameRect {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SceneItem {
    Rect { x: i64, y: i64, width: i64, height: i64, background_color: String, opacity: f64 },
    Text {
        x: i64,
        y: i64,
        text: String,
        color: String,
        font_px: i64,
        font_family: String,
        underline: bool,
        opacity: f64,
        href: Option<String>,
        target: Option<String>,
    },
    Image {
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        src: String,
        alt: String,
        opacity: f64,
        href: Option<String>,
        target: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct FrameViewModel {
    pub id: String,
    pub name: Option<String>,
    pub current_url: String,
    pub title: String,
    pub diagnostics: Vec<String>,
    pub rect: FrameRect,
    pub content_size: ContentSize,
    pub scene_items: Vec<SceneItem>,
    pub html_content: Option<String>,
    pub child_frames: Vec<FrameViewModel>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PageViewModel {
    pub current_url: String,
    pub title: String,
    pub diagnostics: Vec<String>,
    pub content_size: ContentSize,
    pub scene_items: Vec<SceneItem>,
    pub root_frame: FrameViewModel,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavigationState {
    pub can_back: bool,
    pub can_forward: bool,
    pub current_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TabSummary {
    pub id: u32,
    pub title: String,
    pub url: Option<String>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ErrorMetric {
    pub code: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct NavigationEvent {
    pub command: String,
    pub url: Option<String>,
    pub duration_ms: u64,
    pub success: bool,
    pub error_code: Option<String>,
    pub recorded_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct AppMetricsSnapshot {
    pub total_navigations: u64,
    pub successful_navigations: u64,
    pub failed_navigations: u64,
    pub average_duration_ms: u64,
    pub last_duration_ms: Option<u64>,
    pub error_counts: Vec<ErrorMetric>,
    pub recent_events: Vec<NavigationEvent>,
}

pub trait AppService {
    fn open_url(&mut self, url: &str) -> AppResult<PageViewModel>;
    fn activate_link(&mut self, frame_id: &str, href: &str, target: Option<&str>) -> AppResult<PageViewModel>;
    fn reload(&mut self) -> AppResult<PageViewModel>;
    fn back(&mut self) -> AppResult<PageViewModel>;
    fn forward(&mut self) -> AppResult<PageViewModel>;
    fn get_page_view(&self) -> PageViewModel;
    fn set_viewport(&mut self, width: i64, height: i64) -> AppResult<PageViewModel>;
    fn get_navigation_state(&self) -> NavigationState;
    fn get_metrics(&self) -> AppMetricsSnapshot;
    fn new_tab(&mut self) -> TabSummary;
    fn switch_tab(&mut self, id: u32) -> AppResult<PageViewModel>;
    fn close_tab(&mut self, id: u32) -> AppResult<Vec<TabSummary>>;
    fn list_tabs(&self) -> Vec<TabSummary>;
    fn search(&self, query: &str) -> AppResult<Vec<SearchResult>>;
}

