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
        Self {
            code: "validation_error".to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self {
            code: "network_error".to_string(),
            message: message.into(),
            retryable: true,
        }
    }

    pub fn tls(message: impl Into<String>) -> Self {
        Self {
            code: "tls_error".to_string(),
            message: message.into(),
            retryable: true,
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self {
            code: "parse_error".to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn state(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_state".to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn navigation_guard(message: impl Into<String>) -> Self {
        Self {
            code: "navigation_guard_blocked".to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn recovering(message: impl Into<String>) -> Self {
        Self {
            code: "renderer_recovering".to_string(),
            message: message.into(),
            retryable: true,
        }
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
    Rect {
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        background_color: String,
        opacity: f64,
        z_index: i32,
        clip_rect: Option<(i64, i64, i64, i64)>,
    },
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
        z_index: i32,
        clip_rect: Option<(i64, i64, i64, i64)>,
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
        z_index: i32,
        clip_rect: Option<(i64, i64, i64, i64)>,
    },
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RenderBackendKind {
    #[deprecated(note = "WebView backend is legacy-only; use NativeScene")]
    WebView,
    NativeScene,
}

/// Rendering adapter contract used by `cosmo_app_legacy` to keep backend-specific code replaceable.
///
/// ```rust
/// use cosmo_app_legacy::{FrameViewModel, RenderBackend, RenderBackendKind};
///
/// struct CompatBackend;
///
/// impl RenderBackend for CompatBackend {
///     fn name(&self) -> &'static str {
///         "compat-webview"
///     }
///
///     fn kind_for_frame(&self, frame: &FrameViewModel) -> RenderBackendKind {
///         frame.render_backend.clone()
///     }
/// }
/// ```
pub trait RenderBackend {
    fn name(&self) -> &'static str;

    /// Resolves the backend kind for a frame.
    ///
    /// Render paths should follow the engine pipeline order defined by standards:
    /// HTML LS parsing -> DOM Standard tree mutations -> CSS Display/CSS2 visual
    /// formatting model layout -> paint in scene item order.
    fn kind_for_frame(&self, frame: &FrameViewModel) -> RenderBackendKind;

    /// Returns true when this backend kind represents deprecated WebView usage.
    #[allow(deprecated)]
    fn is_legacy_webview(&self, kind: &RenderBackendKind) -> bool {
        matches!(kind, RenderBackendKind::WebView)
    }
}

/// Script execution contract exposed by `cosmo_app_legacy`.
///
/// ```rust
/// use cosmo_app_legacy::{FrameViewModel, ScriptEngine};
///
/// struct NoopScriptEngine;
///
/// impl ScriptEngine for NoopScriptEngine {
///     fn name(&self) -> &'static str {
///         "noop"
///     }
/// }
/// ```
pub trait ScriptEngine {
    fn name(&self) -> &'static str;

    fn can_execute(&self, _frame: &FrameViewModel) -> bool {
        false
    }
}

/// Security policy contract for navigation and content handling decisions.
///
/// ```rust
/// use cosmo_app_legacy::SecurityPolicy;
///
/// struct AllowAllPolicy;
///
/// impl SecurityPolicy for AllowAllPolicy {
///     fn name(&self) -> &'static str {
///         "allow-all"
///     }
/// }
/// ```
pub trait SecurityPolicy {
    fn name(&self) -> &'static str;

    fn allows_navigation(&self, _current_url: Option<&str>, _target_url: &str) -> bool {
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultRenderBackend;

impl RenderBackend for DefaultRenderBackend {
    fn name(&self) -> &'static str {
        "native-scene"
    }

    fn kind_for_frame(&self, frame: &FrameViewModel) -> RenderBackendKind {
        frame.render_backend.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultScriptEngine;

impl ScriptEngine for DefaultScriptEngine {
    fn name(&self) -> &'static str {
        "disabled"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultSecurityPolicy;

impl SecurityPolicy for DefaultSecurityPolicy {
    fn name(&self) -> &'static str {
        "allow-navigation"
    }
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
    // Rendering strategy hint for the UI layer.
    pub render_backend: RenderBackendKind,
    // Canonical document URL used as a base for resource/link resolution.
    pub document_url: String,
    pub scene_items: Vec<SceneItem>,
    pub render_tree: Option<RenderTreeSnapshot>,
    pub html_content: Option<String>,
    pub child_frames: Vec<FrameViewModel>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RenderTreeSnapshot {
    pub root: Option<RenderNode>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct RenderNode {
    pub kind: RenderNodeKind,
    pub node_name: String,
    pub text: Option<String>,
    pub box_info: RenderBox,
    pub style: ResolvedStyle,
    pub children: Vec<RenderNode>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RenderNodeKind {
    Block,
    Inline,
    Text,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RenderBox {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
    pub content_width: i64,
    pub content_height: i64,
    pub margin: (i64, i64, i64, i64),
    pub padding: (i64, i64, i64, i64),
    pub border: (i64, i64, i64, i64),
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ResolvedStyle {
    pub display: String,
    pub position: String,
    pub color: String,
    pub background_color: String,
    pub font_px: i64,
    pub font_family: String,
    pub opacity: f64,
    pub z_index: i32,
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
    pub current_navigation_type: Option<NavigationType>,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NavigationType {
    Document,
    Hash,
    Redirect,
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
    fn activate_link(
        &mut self,
        frame_id: &str,
        href: &str,
        target: Option<&str>,
    ) -> AppResult<PageViewModel>;
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

    fn render_backend(&self) -> Box<dyn RenderBackend> {
        Box::new(DefaultRenderBackend)
    }

    fn script_engine(&self) -> Box<dyn ScriptEngine> {
        Box::new(DefaultScriptEngine)
    }

    fn security_policy(&self) -> Box<dyn SecurityPolicy> {
        Box::new(DefaultSecurityPolicy)
    }
}
