use crate::error::Error;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::Node;
use crate::renderer::dom::node::NodeKind;
use std::collections::BTreeMap;
use std::format;
use std::rc::Rc;
use std::string::String;
use std::string::ToString;
use std::vec::Vec;
use std::cell::RefCell;

use crate::renderer::css::token::CssToken;

/// Per-element CSS custom-property scope, shared copy-on-write between
/// elements (children clone the Rc; an element that defines its own
/// `--name` values gets a fresh map).
pub type CustomProperties = Rc<BTreeMap<String, Vec<CssToken>>>;

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct EdgeSize {
    top: f64,
    right: f64,
    bottom: f64,
    left: f64,
}

impl EdgeSize {
    pub fn zero() -> Self {
        Self {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }

    pub fn all(value: f64) -> Self {
        Self::from_values(value, value, value, value)
    }

    pub fn from_values(top: f64, right: f64, bottom: f64, left: f64) -> Self {
        Self {
            top,
            right,
            bottom,
            left,
        }
    }

    pub fn horizontal(&self) -> f64 {
        self.left + self.right
    }

    pub fn vertical(&self) -> f64 {
        self.top + self.bottom
    }

    pub fn top(&self) -> f64 {
        self.top
    }

    pub fn left(&self) -> f64 {
        self.left
    }

    pub fn right(&self) -> f64 {
        self.right
    }

    pub fn bottom(&self) -> f64 {
        self.bottom
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ComputedStyle {
    background_color: Option<Color>,
    background_image: Option<String>,
    background_gradient: Option<LinearGradient>,
    color: Option<Color>,
    display: Option<DisplayType>,
    float: Option<Float>,
    clear: Option<Clear>,
    transitions: Vec<TransitionSpec>,
    font_family: Option<String>,
    font_size: Option<FontSize>,
    text_decoration: Option<TextDecoration>,
    bold: Option<bool>,
    visibility_hidden: Option<bool>,
    box_sizing_border_box: Option<bool>,
    /// `grid-template-areas` rows (each row a list of area names; "." = empty).
    /// `list-style-type` (inherited; UA default disc for <ul>, decimal for
    /// <ol> subtrees).
    list_style_type: Option<ListStyleType>,
    grid_template_areas: Option<Rc<Vec<Vec<String>>>>,
    /// Named grid lines of grid-template-columns: entry i = names before
    /// track i (last entry = names after the final track).
    grid_column_line_names: Option<Rc<Vec<Vec<String>>>>,
    /// `grid-area: <name>` on a grid item.
    grid_area_name: Option<String>,
    flex_grow: Option<f64>,
    flex_shrink: Option<f64>,
    /// `flex-basis` in px; None = auto (use width, else content size).
    flex_basis: Option<f64>,
    justify_content: Option<JustifyContent>,
    align_items: Option<AlignItems>,
    align_self: Option<AlignItems>,
    min_width: Option<SizeLimit>,
    max_width: Option<SizeLimit>,
    min_height: Option<SizeLimit>,
    max_height: Option<SizeLimit>,
    opacity: Option<f64>,
    /// Used (animated) opacity while a CSS transition is running on this box.
    /// The cascade's `opacity` stays the *target*; the transition driver writes
    /// the interpolated value to a `data-cosmo-anim-opacity` attribute, which
    /// `defaulting` resolves into this field. `None` = not animating.
    anim_opacity: Option<f64>,
    /// As `anim_opacity`, for a running `background-color` transition.
    anim_background_color: Option<Color>,
    /// As `anim_opacity`, for a running `color` transition. `color` inherits,
    /// so this also carries an animating ancestor's used value down to
    /// descendants that declared no color of their own.
    anim_color: Option<Color>,
    /// Cascade `width`/`height` saved before an animation override replaced
    /// them. Unlike the other animated properties, sizes are overridden *in
    /// place* — every consumer (min/max clamps, box-sizing, layout) then sees
    /// the animated value with no further plumbing — so the target the driver
    /// diffs against is stashed here instead.
    width_target: Option<f64>,
    height_target: Option<f64>,
    height: Option<f64>,
    height_ratio: Option<f64>,
    width: Option<f64>,
    width_ratio: Option<f64>,
    /// True when width/height came from an author declaration (the
    /// defaulting pass fills the Options with 0.0 for every element, so
    /// Some(0.0) alone can't mean "author wrote 0").
    width_author: bool,
    height_author: bool,
    margin_left_auto: bool,
    margin_right_auto: bool,
    margin: Option<EdgeSize>,
    padding: Option<EdgeSize>,
    border: Option<EdgeSize>,
    /// Visual color for border strokes (set from HTML `border` attribute or CSS).
    /// None means no visible border stroke even if border-width > 0.
    border_color: Option<Color>,
    position: Option<PositionType>,
    offset_top: Option<f64>,
    offset_left: Option<f64>,
    /// Author actually declared top/left (the defaulting pass fills the
    /// Options with 0.0, so Some alone can't mean "declared"). An absolute
    /// box with an auto side keeps its static position on that axis.
    offset_top_author: bool,
    offset_left_author: bool,
    /// CSS `right`/`bottom` — used by the fixed-position pass to anchor a box
    /// against the viewport's far edges.
    offset_right: Option<f64>,
    offset_bottom: Option<f64>,
    /// Percentage forms of top/left (fraction of the containing block),
    /// resolved at positioning time.
    offset_top_ratio: Option<f64>,
    offset_left_ratio: Option<f64>,
    text_align: Option<TextAlign>,
    /// True when `text_align` originates from a legacy presentational center
    /// hint (a `<center>` element or `align="center"` attribute) rather than a
    /// CSS `text-align` declaration. Legacy center aligns block boxes but is
    /// reset to the start edge inside table cells, whereas CSS `text-align`
    /// inherits into cells normally.
    text_align_legacy: bool,
    z_index: Option<i32>,
    overflow_clip: Option<bool>,
    /// `flex-direction` of a flex container (`display:flex`). Only meaningful
    /// when `display` is `Flex`; controls whether children are laid out along
    /// the row (main = horizontal) or column (main = vertical) axis.
    flex_direction: Option<FlexDirection>,
    /// `background-position` as (x, x_is_percent, y, y_is_percent). Percent
    /// values resolve against (box size − image size) at paint time; pixel
    /// values may be negative (sprite sheets).
    background_position: Option<(f64, bool, f64, bool)>,
    /// `background-repeat: no-repeat` (default false = repeat).
    background_no_repeat: bool,
    /// `background-size` as (mode, w, w_is_percent, h, h_is_percent):
    /// mode 0 = explicit (a negative dimension means `auto` for that axis),
    /// mode 1 = `cover`, mode 2 = `contain` (w/h unused for 1 and 2).
    background_size: Option<(u8, f64, bool, f64, bool)>,
    /// `line-height`. Inherited.
    line_height: Option<LineHeight>,
    /// Sticky scroll context stamped by the post-layout pass onto every node
    /// of a sticky subtree: (top threshold, the sticky box's laid-out y,
    /// max pin delta). The painter clamps the subtree's scroll so the box
    /// pins at the threshold once the page scrolls past it, releasing again
    /// after `max_delta` (the containing block's bottom).
    sticky_context: Option<(f64, f64, f64)>,
    /// True for every node inside a position:fixed subtree (stamped by the
    /// post-layout pass): descendants share the fixed box's scroll exemption
    /// and stacking level even though their own position is Static.
    fixed_subtree: bool,
    /// Final paint-order key stamped by the post-layout pass: the root canvas
    /// sits at −2M, normal flow at 0, stacking contexts at ±1M+z (nested
    /// contexts offset within the parent's bucket). Mappers feed this to the
    /// painter's z sort.
    paint_z: Option<i32>,
    /// True when overflow is scroll/auto (an interactive scroll container);
    /// hidden/clip also clip but cannot be scrolled.
    overflow_scrollable: bool,
    /// `transform` is declared with a non-none value (stacking trigger).
    has_transform: bool,
    /// Parsed transform: (tx, tx_is_percent, ty, ty_is_percent, scale).
    /// Percentages resolve against the box's own size at the post-layout
    /// pass; scale is uniform (anisotropic scale uses the x factor).
    transform_op: Option<(f64, bool, f64, bool, f64)>,
    /// Parsed `rotate(<deg>)` in degrees (clockwise), if any.
    transform_rotate: Option<f64>,
    /// Scale context stamped on a scaled subtree: (origin_x, origin_y,
    /// factor). Mappers scale command geometry and font sizes through it.
    scale_context: Option<(f64, f64, f64)>,
    /// Rotation context stamped on a rotated subtree: (center_x, center_y,
    /// degrees) in page coordinates. The painter rotates DrawRect fills about
    /// the center; text/image anchors are rotated about it so they travel
    /// with the box (glyphs stay axis-aligned — an approximation).
    rotate_context: Option<(f64, f64, f64)>,
    /// `border-radius` in pixels (single radius, all corners).
    border_radius: Option<f64>,
    /// `box-shadow`: (dx, dy, blur, color).
    box_shadow: Option<(f64, f64, f64, Color)>,
    /// `white-space: nowrap` — suppress line wrapping. Inherited.
    white_space: Option<WhiteSpace>,
    text_transform: Option<TextTransform>,
    /// `text-overflow: ellipsis` — truncate a clipped single line with `…`.
    text_overflow_ellipsis: bool,
    /// Final clip rectangle (x, y, w, h) stamped by the post-layout pass:
    /// the intersection of every overflow-clipping ancestor box (and this
    /// box itself when it clips). Page coordinates.
    final_clip: Option<(f64, f64, f64, f64)>,
    /// Nearest scroll-container id this box's CONTENT belongs to (stamped):
    /// the renderer offsets these commands by the container's inner scroll.
    scroll_container: Option<u32>,
    /// Set on the scroll container's own box: (id, content width, content
    /// height) — lets the renderer register the scrollable region and clamp
    /// its offsets on both axes.
    scroll_container_def: Option<(u32, f64, f64)>,
    /// Column tracks from `grid-template-columns`. Only meaningful when
    /// `display` is `Grid`.
    grid_template_columns: Option<Vec<GridTrack>>,
    /// `column-gap` / `row-gap` (or the `gap` shorthand) in pixels.
    column_gap: Option<f64>,
    row_gap: Option<f64>,
    /// CSS custom properties (`--name`) in scope for this element: inherited
    /// from the parent, overridden by the element's own definitions.
    /// https://www.w3.org/TR/css-variables-1/#cycles
    custom_properties: Option<CustomProperties>,
}

/// `line-height` value. https://www.w3.org/TR/CSS22/visudet.html#line-height
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineHeight {
    /// Fixed pixel height (from a length).
    Px(f64),
    /// Multiplier of the element's own font size (from a number or %).
    Factor(f64),
}

/// One column track of a grid template.
/// https://www.w3.org/TR/css-grid-1/#track-sizing
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GridTrack {
    /// Fixed size in pixels (from px/pt/… lengths).
    Px(f64),
    /// Flexible fraction of the remaining space (`1fr`).
    Fr(f64),
    /// `auto` / unsupported sizes — treated as `1fr`.
    Auto,
}

impl ComputedStyle {
    pub fn new() -> Self {
        Self {
            background_color: None,
            background_image: None,
            background_gradient: None,
            color: None,
            display: None,
            float: None,
            clear: None,
            transitions: Vec::new(),
            font_family: None,
            font_size: None,
            text_decoration: None,
            bold: None,
            visibility_hidden: None,
            box_sizing_border_box: None,
            list_style_type: None,
            grid_template_areas: None,
            grid_column_line_names: None,
            grid_area_name: None,
            flex_grow: None,
            flex_shrink: None,
            flex_basis: None,
            justify_content: None,
            align_items: None,
            align_self: None,
            min_width: None,
            max_width: None,
            min_height: None,
            max_height: None,
            opacity: None,
            anim_opacity: None,
            anim_background_color: None,
            anim_color: None,
            width_target: None,
            height_target: None,
            height: None,
            height_ratio: None,
            width: None,
            width_ratio: None,
            width_author: false,
            height_author: false,
            margin_left_auto: false,
            margin_right_auto: false,
            margin: None,
            padding: None,
            border: None,
            border_color: None,
            position: None,
            offset_top: None,
            offset_left: None,
            offset_top_author: false,
            offset_left_author: false,
            offset_right: None,
            offset_bottom: None,
            offset_top_ratio: None,
            offset_left_ratio: None,
            text_align: None,
            text_align_legacy: false,
            z_index: None,
            overflow_clip: None,
            flex_direction: None,
            background_position: None,
            background_no_repeat: false,
            background_size: None,
            line_height: None,
            sticky_context: None,
            fixed_subtree: false,
            paint_z: None,
            overflow_scrollable: false,
            has_transform: false,
            transform_op: None,
            transform_rotate: None,
            scale_context: None,
            rotate_context: None,
            border_radius: None,
            box_shadow: None,
            white_space: None,
            text_transform: None,
            text_overflow_ellipsis: false,
            final_clip: None,
            scroll_container: None,
            scroll_container_def: None,
            grid_template_columns: None,
            column_gap: None,
            row_gap: None,
            custom_properties: None,
        }
    }

    pub fn defaulting(&mut self, node: &Rc<RefCell<Node>>, parent_style: Option<ComputedStyle>) {
        // Handle HTML align attribute (presentational hint) BEFORE inheritance so
        // that an explicit align="left" / "right" on an element can prevent the
        // inherited text-align (e.g. from a <center> ancestor) from taking over.
        // Spec: HTML Living Standard §14.3 — presentational hints.
        // https://html.spec.whatwg.org/multipage/rendering.html#tables-2
        let is_table = node.borrow().element_kind() == Some(ElementKind::Table);
        // Own bgcolor attribute must be applied BEFORE background inheritance,
        // or an inherited ancestor background (e.g. <table bgcolor=...>) fills
        // the slot first and the element's own bgcolor is ignored.
        if self.background_color.is_none() {
            if let Some(bgcolor) = get_element_attribute(node, "bgcolor") {
                if let Some(color) = parse_html_color(&bgcolor) {
                    self.background_color = Some(color);
                }
            }
        }
        if let Some(align) = get_element_attribute(node, "align") {
            if align.eq_ignore_ascii_case("center") {
                self.margin_left_auto = true;
                self.margin_right_auto = true;
                // align="center" on a TABLE centers the table box (margin auto),
                // but does NOT set text-align:center on cell contents.
                if !is_table && self.text_align.is_none() {
                    self.text_align = Some(TextAlign::Center);
                    self.text_align_legacy = true;
                }
            } else if align.eq_ignore_ascii_case("right") {
                if self.text_align.is_none() {
                    self.text_align = Some(TextAlign::Right);
                }
            } else if align.eq_ignore_ascii_case("left") {
                // Unconditionally set Left: HTML align="left" must override any
                // inherited text-align:center from a <center> ancestor.
                self.text_align = Some(TextAlign::Left);
            }
        }

        // The parent's *used* vs *target* opacity: a running transition on an
        // ancestor scales this subtree's used opacity without disturbing the
        // cascade values the driver compares against (see `anim_opacity`).
        let parent_opacity_target = parent_style.as_ref().map(|p| p.opacity_or_default());
        let parent_opacity_used = parent_style.as_ref().map(|p| p.used_opacity());
        // `color` inherits, so an animating ancestor's *used* color has to flow
        // to descendants that declare none of their own. Captured before the
        // inheritance block below fills `self.color` from the parent's target.
        let color_declared = self.color.is_some();
        let parent_anim_color = parent_style.as_ref().and_then(|p| p.anim_color.clone());

        if let Some(parent_style) = parent_style {
            // NOTE: background-color is NOT inherited in CSS. The transparent
            // default lets the parent's background show through; inheriting a
            // concrete color here created opaque copies on every descendant
            // box, which then painted over negative-z-index layers.
            if self.color.is_none() && parent_style.color() != Color::black() {
                self.color = Some(parent_style.color());
            }
            if self.font_family.is_none() {
                self.font_family = Some(parent_style.font_family());
            }
            if self.font_size.is_none() && parent_style.font_size() != FontSize::Medium {
                self.font_size = Some(parent_style.font_size());
            }
            if self.text_decoration.is_none()
                && parent_style.text_decoration() != TextDecoration::None
            {
                self.text_decoration = Some(parent_style.text_decoration());
            }
            // font-weight (bold) is inherited.
            if self.bold.is_none() && parent_style.bold == Some(true) {
                self.bold = Some(true);
            }
            // visibility is inherited (a child may set visible to reappear
            // inside a hidden parent).
            if self.visibility_hidden.is_none() {
                self.visibility_hidden = parent_style.visibility_hidden;
            }
            // list-style-type is inherited (set on <ul>/<ol>, read on <li>).
            if self.list_style_type.is_none() {
                self.list_style_type = parent_style.list_style_type;
            }
            // line-height is inherited.
            if self.line_height.is_none() {
                self.line_height = parent_style.line_height;
            }
            // white-space is inherited.
            if self.text_transform.is_none() {
                self.text_transform = parent_style.text_transform;
            }
            if self.white_space.is_none() {
                self.white_space = parent_style.white_space;
            }
            // text-align is inherited.
            if self.text_align.is_none() && parent_style.text_align != Some(TextAlign::Left) {
                // A legacy presentational center (from a <center> element or an
                // align="center" attribute) centers block boxes but is reset to
                // the start edge inside table cells — it does not inherit as
                // text-align into cell content. CSS `text-align:center`, which
                // carries no legacy flag, inherits into cells normally.
                let into_cell = matches!(
                    node.borrow().element_kind(),
                    Some(ElementKind::Td) | Some(ElementKind::Th)
                );
                if into_cell
                    && parent_style.text_align == Some(TextAlign::Center)
                    && parent_style.text_align_legacy
                {
                    self.text_align = Some(TextAlign::Left);
                } else {
                    self.text_align = parent_style.text_align;
                    self.text_align_legacy = parent_style.text_align_legacy;
                }
            }
            let parent_opacity = parent_style.opacity();
            if let Some(opacity) = self.opacity {
                self.opacity = Some((opacity * parent_opacity).clamp(0.0, 1.0));
            } else if parent_opacity < 1.0 {
                self.opacity = Some(parent_opacity);
            }
        }

        // (The bgcolor presentational hint is applied before inheritance, at
        // the top of this function.)
        // Handle HTML <body background="..."> attribute for tiled background image.
        if self.background_image.is_none() {
            if let Some(bg) = get_element_attribute(node, "background") {
                if !bg.is_empty() {
                    self.background_image = Some(bg);
                }
            }
        }

        if self.color.is_none() {
            // <body text="..."> or <font color="...">
            let color_attr = get_element_attribute(node, "text")
                .or_else(|| get_element_attribute(node, "color"));
            if let Some(color_val) = color_attr {
                if let Some(color) = parse_html_color(&color_val) {
                    self.color = Some(color);
                }
            }
        }

        // HR always renders with its own gray background, ignoring parent inheritance.
        if node.borrow().element_kind() == Some(ElementKind::Hr) {
            self.background_color = Some(Color::gray());
        }
        if self.background_color.is_none() {
            self.background_color = Some(match node.borrow().element_kind() {
                Some(ElementKind::Button) | Some(ElementKind::Img) | Some(ElementKind::Input) => {
                    Color::lightgray()
                }
                Some(ElementKind::Body) => Color::white(),
                // Use transparent default so parent backgrounds (e.g. body bgcolor) show through.
                _ => Color::transparent(),
            });
        }
        if self.color.is_none() {
            if node.borrow().element_kind() == Some(ElementKind::A) {
                self.color = Some(Color::link_blue());
            } else {
                self.color = Some(Color::black());
            }
        }
        if self.display.is_none() {
            self.display = Some(DisplayType::default(node));
        }
        if self.font_family.is_none() {
            self.font_family = Some("serif".to_string());
        }
        if self.font_size.is_none() {
            self.font_size = Some(FontSize::default(node));
        }
        if self.text_decoration.is_none() {
            self.text_decoration = Some(TextDecoration::default(node));
        }
        if self.bold.is_none() {
            // UA stylesheet: <strong>, <b>, <h1>-<h3> are bold by default.
            let is_bold = matches!(
                node.borrow().element_kind(),
                Some(ElementKind::Strong)
                    | Some(ElementKind::B)
                    | Some(ElementKind::H1)
                    | Some(ElementKind::H2)
                    | Some(ElementKind::H3)
            );
            self.bold = Some(is_bold);
        }
        if self.opacity.is_none() {
            self.opacity = Some(1.0);
        }
        if self.list_style_type.is_none() {
            // UA stylesheet: list containers seed the inherited marker type.
            self.list_style_type = match node.borrow().element_tag_name().as_deref() {
                Some("ul") | Some("menu") | Some("dir") => Some(ListStyleType::Disc),
                Some("ol") => Some(ListStyleType::Decimal),
                _ => None,
            };
        }
        if self.white_space.is_none()
            && node.borrow().element_tag_name().as_deref() == Some("pre")
        {
            self.white_space = Some(WhiteSpace::Pre);
        }
        // UA stylesheet: lists indent their items so outside markers have
        // room (browsers use padding-inline-start: 40px).
        if self.padding.is_none()
            && matches!(
                node.borrow().element_tag_name().as_deref(),
                Some("ul") | Some("ol") | Some("menu") | Some("dir")
            )
        {
            self.padding = Some(EdgeSize::from_values(0.0, 0.0, 0.0, 40.0));
        }
        if self.height.is_none() {
            self.height = Some(0.0);
        }
        if self.width.is_none() {
            self.width = Some(0.0);
        }
        self.resolve_animated_values(
            node,
            parent_opacity_target,
            parent_opacity_used,
            color_declared,
            parent_anim_color,
        );
        // <center> tag implies text-align: center for children.
        if node.borrow().element_kind() == Some(ElementKind::Center) && self.text_align.is_none() {
            self.text_align = Some(TextAlign::Center);
            self.text_align_legacy = true;
        }
        // Block children of <center> should be horizontally centered (margin auto).
        {
            let parent_is_center = node
                .borrow()
                .parent()
                .upgrade()
                .map(|p| p.borrow().element_kind() == Some(ElementKind::Center))
                .unwrap_or(false);
            if parent_is_center {
                if let NodeKind::Element(ref e) = node.borrow().kind() {
                    if e.is_block_element() {
                        self.margin_left_auto = true;
                        self.margin_right_auto = true;
                    }
                }
            }
        }

        if self.margin.is_none() {
            // UA stylesheet defaults (CSS2 §6.4 — browser default styles).
            match node.borrow().element_kind() {
                Some(ElementKind::Hr) => {
                    self.margin = Some(EdgeSize::from_values(8.0, 0.0, 8.0, 0.0));
                }
                Some(ElementKind::P) => {
                    // Browsers give <p> 1em top + 1em bottom margin by default.
                    self.margin = Some(EdgeSize::from_values(16.0, 0.0, 16.0, 0.0));
                }
                Some(ElementKind::H1) => {
                    self.margin = Some(EdgeSize::from_values(21.0, 0.0, 21.0, 0.0));
                }
                Some(ElementKind::H2) => {
                    self.margin = Some(EdgeSize::from_values(19.0, 0.0, 19.0, 0.0));
                }
                Some(ElementKind::H3) => {
                    self.margin = Some(EdgeSize::from_values(17.0, 0.0, 17.0, 0.0));
                }
                Some(ElementKind::Tr) => {
                    // HTML4 cellspacing: add a gap above each table row so that
                    // adjacent cell borders don't visually merge into a double line.
                    // Default cellspacing = 2 per HTML4.01 §11.3.3.
                    // https://www.w3.org/TR/html4/struct/tables.html#adef-cellspacing
                    let cs = find_ancestor_table_cellspacing(node);
                    self.margin = Some(EdgeSize::from_values(cs as f64, 0.0, 0.0, 0.0));
                }
                Some(ElementKind::Caption) => {
                    // <caption> gets small top/bottom margin to separate it from the table rows.
                    self.margin = Some(EdgeSize::from_values(4.0, 0.0, 4.0, 0.0));
                }
                _ => {
                    self.margin = Some(EdgeSize::zero());
                }
            }
        }
        if self.padding.is_none() {
            // Default padding-left for list containers (UA stylesheet).
            if node.borrow().element_kind() == Some(ElementKind::Ul) {
                self.padding = Some(EdgeSize::from_values(0.0, 0.0, 0.0, 40.0));
            } else {
                self.padding = Some(EdgeSize::zero());
            }
        }
        if self.position.is_none() {
            self.position = Some(PositionType::Static);
        }
        // HTML `border` presentational hint on <table> and its cells.
        // Spec: HTML Living Standard §14.3 — presentational hints for tables.
        // <TABLE BORDER=N> sets a N-px border on the table, and a 1-px border
        // on every <td>/<th> inside it.
        if self.border.is_none() {
            let elem_kind = node.borrow().element_kind();
            if elem_kind == Some(ElementKind::Table) {
                if let Some(border_str) = get_element_attribute(node, "border") {
                    let px: f64 = if border_str.trim().is_empty() {
                        1.0 // bare `border` attribute with no value → 1
                    } else {
                        border_str.trim().parse().unwrap_or(0.0)
                    };
                    if px > 0.0 {
                        self.border = Some(EdgeSize::all(px));
                        if self.border_color.is_none() {
                            self.border_color = Some(Color::from_code("#808080").unwrap());
                        }
                    }
                }
            } else if elem_kind == Some(ElementKind::Td) || elem_kind == Some(ElementKind::Th) {
                if find_ancestor_table_border(node) > 0 {
                    self.border = Some(EdgeSize::all(1.0));
                    if self.border_color.is_none() {
                        self.border_color = Some(Color::from_code("#808080").unwrap());
                    }
                }
            }
        }
        if self.border.is_none() {
            self.border = Some(EdgeSize::zero());
        }
        if self.offset_top.is_none() {
            self.offset_top = Some(0.0);
        }
        if self.offset_left.is_none() {
            self.offset_left = Some(0.0);
        }
        if self.text_align.is_none() {
            // <caption> is centered by default per CSS 2.2 table spec.
            if node.borrow().element_kind() == Some(ElementKind::Caption) {
                self.text_align = Some(TextAlign::Center);
            } else {
                self.text_align = Some(TextAlign::Left);
            }
        }
        // z_index intentionally stays None when not declared: `auto` is
        // distinguishable from an explicit 0 (auto-positioned boxes do not
        // form stacking contexts; their children escape to the parent
        // context). Use z_index_or_default() to read it.

        if self.overflow_clip.is_none() {
            self.overflow_clip = Some(false);
        }
    }

    pub fn set_background_color(&mut self, color: Color) {
        self.background_color = Some(color);
    }

    pub fn background_color(&self) -> Color {
        self.background_color
            .clone()
            .expect("failed to access CSS property: background-color")
    }

    pub fn background_image(&self) -> Option<&str> {
        self.background_image.as_deref()
    }

    pub fn background_gradient(&self) -> Option<&LinearGradient> {
        self.background_gradient.as_ref()
    }
    pub fn set_background_gradient(&mut self, g: LinearGradient) {
        self.background_gradient = Some(g);
    }

    pub fn set_background_image(&mut self, url: String) {
        self.background_image = Some(url);
    }

    pub fn set_background_position(&mut self, x: f64, x_pct: bool, y: f64, y_pct: bool) {
        self.background_position = Some((x, x_pct, y, y_pct));
    }

    pub fn background_position(&self) -> Option<(f64, bool, f64, bool)> {
        self.background_position
    }

    pub fn set_background_no_repeat(&mut self, no_repeat: bool) {
        self.background_no_repeat = no_repeat;
    }

    pub fn set_background_size(&mut self, size: (u8, f64, bool, f64, bool)) {
        self.background_size = Some(size);
    }

    pub fn set_line_height(&mut self, lh: LineHeight) {
        self.line_height = Some(lh);
    }

    pub fn line_height(&self) -> Option<LineHeight> {
        self.line_height
    }

    pub fn set_sticky_context(&mut self, top: f64, container_y: f64, max_delta: f64) {
        self.sticky_context = Some((top, container_y, max_delta));
    }

    pub fn sticky_context(&self) -> Option<(f64, f64, f64)> {
        self.sticky_context
    }

    pub fn set_fixed_subtree(&mut self) {
        self.fixed_subtree = true;
    }

    pub fn fixed_subtree(&self) -> bool {
        self.fixed_subtree
    }

    pub fn set_paint_z(&mut self, z: i32) {
        self.paint_z = Some(z);
    }

    pub fn paint_z(&self) -> i32 {
        self.paint_z.unwrap_or(0)
    }

    /// z-index without panicking on un-defaulted styles.
    pub fn z_index_or_default(&self) -> i32 {
        self.z_index.unwrap_or(0)
    }

    /// True when z-index was declared (i.e. not `auto`).
    pub fn z_index_specified(&self) -> bool {
        self.z_index.is_some()
    }

    pub fn set_overflow_scrollable(&mut self, v: bool) {
        self.overflow_scrollable = v;
    }

    pub fn overflow_scrollable(&self) -> bool {
        self.overflow_scrollable
    }

    pub fn set_has_transform(&mut self, v: bool) {
        self.has_transform = v;
    }

    pub fn has_transform(&self) -> bool {
        self.has_transform
    }

    pub fn set_transform_op(&mut self, op: (f64, bool, f64, bool, f64)) {
        self.transform_op = Some(op);
    }

    pub fn transform_op(&self) -> Option<(f64, bool, f64, bool, f64)> {
        self.transform_op
    }

    pub fn set_transform_rotate(&mut self, deg: f64) {
        self.transform_rotate = Some(deg);
    }

    pub fn transform_rotate(&self) -> Option<f64> {
        self.transform_rotate
    }

    pub fn set_rotate_context(&mut self, cx: f64, cy: f64, deg: f64) {
        self.rotate_context = Some((cx, cy, deg));
    }

    pub fn rotate_context(&self) -> Option<(f64, f64, f64)> {
        self.rotate_context
    }

    pub fn set_scale_context(&mut self, ox: f64, oy: f64, factor: f64) {
        self.scale_context = Some((ox, oy, factor));
    }

    pub fn scale_context(&self) -> Option<(f64, f64, f64)> {
        self.scale_context
    }

    pub fn set_border_radius(&mut self, r: f64) {
        self.border_radius = Some(r.max(0.0));
    }

    pub fn border_radius(&self) -> f64 {
        self.border_radius.unwrap_or(0.0)
    }

    pub fn set_box_shadow(&mut self, dx: f64, dy: f64, blur: f64, color: Color) {
        self.box_shadow = Some((dx, dy, blur, color));
    }

    pub fn box_shadow(&self) -> Option<(f64, f64, f64, Color)> {
        self.box_shadow.clone()
    }

    pub fn set_white_space(&mut self, v: WhiteSpace) {
        self.white_space = Some(v);
    }

    pub fn white_space(&self) -> WhiteSpace {
        self.white_space.unwrap_or(WhiteSpace::Normal)
    }

    pub fn text_transform(&self) -> TextTransform {
        self.text_transform.unwrap_or(TextTransform::None)
    }
    pub fn set_text_transform(&mut self, v: TextTransform) {
        self.text_transform = Some(v);
    }

    /// Automatic wrapping at spaces is suppressed (nowrap/pre).
    pub fn white_space_nowrap(&self) -> bool {
        matches!(self.white_space(), WhiteSpace::Nowrap | WhiteSpace::Pre)
    }

    /// Runs of spaces/tabs are preserved (pre/pre-wrap).
    pub fn white_space_preserves_spaces(&self) -> bool {
        matches!(self.white_space(), WhiteSpace::Pre | WhiteSpace::PreWrap)
    }

    /// Newlines force line breaks (pre/pre-wrap/pre-line).
    pub fn white_space_preserves_newlines(&self) -> bool {
        matches!(
            self.white_space(),
            WhiteSpace::Pre | WhiteSpace::PreWrap | WhiteSpace::PreLine
        )
    }

    pub fn set_text_overflow_ellipsis(&mut self, v: bool) {
        self.text_overflow_ellipsis = v;
    }

    pub fn text_overflow_ellipsis(&self) -> bool {
        self.text_overflow_ellipsis
    }

    /// Opacity without panicking on un-defaulted styles. This is the *target*
    /// (cascade) value — the transition driver compares against it to notice a
    /// changed target. Painting uses [`used_opacity`].
    pub fn opacity_or_default(&self) -> f64 {
        self.opacity.unwrap_or(1.0)
    }

    /// The opacity actually painted: the in-flight transition's interpolated
    /// value when one is running on this box (or inherited from an animating
    /// ancestor), else the cascade value.
    /// Spec: CSS Transitions L1 §3 — a running transition overrides the
    /// declared value for the duration. https://www.w3.org/TR/css-transitions-1/
    pub fn used_opacity(&self) -> f64 {
        self.anim_opacity.unwrap_or_else(|| self.opacity_or_default())
    }

    /// Whether a transition override is in effect on this box.
    pub fn has_animated_opacity(&self) -> bool {
        self.anim_opacity.is_some()
    }

    /// The background color actually painted (see [`used_opacity`]).
    pub fn used_background_color(&self) -> Color {
        self.anim_background_color
            .clone()
            .unwrap_or_else(|| self.background_color())
    }

    /// The text color actually painted (see [`used_opacity`]).
    pub fn used_color(&self) -> Color {
        self.anim_color.clone().unwrap_or_else(|| self.color())
    }

    /// The declared (cascade) value of `property` — the target a transition
    /// animates towards. Never the animated override.
    pub fn animated_target(&self, property: AnimatedProperty) -> AnimatedValue {
        match property {
            AnimatedProperty::Opacity => AnimatedValue::Number(self.opacity_or_default()),
            AnimatedProperty::BackgroundColor => {
                let (r, g, b, a) = self.background_color().rgba_channels();
                AnimatedValue::Rgba(r, g, b, a)
            }
            AnimatedProperty::Color => {
                let (r, g, b, a) = self.color().rgba_channels();
                AnimatedValue::Rgba(r, g, b, a)
            }
            AnimatedProperty::Width => {
                AnimatedValue::Number(self.width_target.unwrap_or_else(|| self.width()))
            }
            AnimatedProperty::Height => {
                AnimatedValue::Number(self.height_target.unwrap_or_else(|| self.height()))
            }
        }
    }

    /// Whether `property` currently has an interpolable value on this box.
    /// Sizes only qualify when the author gave a definite length: `auto` and
    /// percentages have no numeric value to animate between.
    /// Spec: CSS Transitions L1 §2 — only values of an animatable *type* with
    /// both endpoints interpolable start a transition.
    pub fn animatable(&self, property: AnimatedProperty) -> bool {
        match property {
            AnimatedProperty::Width => self.width_author && self.width_ratio.is_none(),
            AnimatedProperty::Height => self.height_author && self.height_ratio.is_none(),
            _ => true,
        }
    }

    /// Resolve the `data-cosmo-anim-*` overrides (written by the runtime's
    /// transition driver) into the used values. Opacity additionally propagates
    /// an animating ancestor's override down as a ratio so descendants fade
    /// with it — mirroring how the cascade multiplies inherited opacity.
    fn resolve_animated_values(
        &mut self,
        node: &Rc<RefCell<Node>>,
        parent_target: Option<f64>,
        parent_used: Option<f64>,
        color_declared: bool,
        parent_anim_color: Option<Color>,
    ) {
        let own = get_element_attribute(node, ANIM_OPACITY_ATTR)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .map(|v| v.clamp(0.0, 1.0));
        let parent_target = parent_target.unwrap_or(1.0);
        let parent_used = parent_used.unwrap_or(1.0);
        // How much the ancestors' running transitions scale this subtree. The
        // cascade already folded `parent_target` into `self.opacity`, so only
        // the ancestor's deviation from its target is applied here.
        let ancestor_ratio = if (parent_used - parent_target).abs() <= f64::EPSILON {
            1.0
        } else if parent_target > 0.0 {
            parent_used / parent_target
        } else {
            // A fully transparent target can't be scaled; use the used value.
            parent_used
        };
        self.anim_opacity = match own {
            // The attribute holds a specified (pre-inheritance) value.
            Some(o) => Some((o * parent_used).clamp(0.0, 1.0)),
            None if ancestor_ratio != 1.0 => {
                Some((self.opacity_or_default() * ancestor_ratio).clamp(0.0, 1.0))
            }
            None => None,
        };
        // background-color doesn't inherit, so its override is purely local.
        self.anim_background_color =
            get_element_attribute(node, ANIM_BACKGROUND_COLOR_ATTR)
                .and_then(|v| Color::from_code(v.trim()).ok());
        // color does inherit: without an override of its own, an element that
        // declared no color follows an animating ancestor's used value.
        self.anim_color = get_element_attribute(node, ANIM_COLOR_ATTR)
            .and_then(|v| Color::from_code(v.trim()).ok())
            .or(if color_declared { None } else { parent_anim_color });

        // Sizes are overridden in place (see `width_target`): the cascade value
        // steps aside as the target and the animated length becomes the used
        // one, so clamps/box-sizing/layout need no knowledge of animation.
        for (attr, target, used, author) in [
            (
                ANIM_WIDTH_ATTR,
                &mut self.width_target,
                &mut self.width,
                &mut self.width_author,
            ),
            (
                ANIM_HEIGHT_ATTR,
                &mut self.height_target,
                &mut self.height,
                &mut self.height_author,
            ),
        ] {
            let Some(animated) = get_element_attribute(node, attr)
                .and_then(|v| v.trim().parse::<f64>().ok())
                .filter(|v| v.is_finite() && *v >= 0.0)
            else {
                *target = None;
                continue;
            };
            *target = *used;
            *used = Some(animated);
            *author = true;
        }
    }

    pub fn set_final_clip(&mut self, clip: (f64, f64, f64, f64)) {
        self.final_clip = Some(clip);
    }

    pub fn final_clip(&self) -> Option<(f64, f64, f64, f64)> {
        self.final_clip
    }

    pub fn set_scroll_container(&mut self, id: u32) {
        self.scroll_container = Some(id);
    }

    pub fn scroll_container(&self) -> Option<u32> {
        self.scroll_container
    }

    pub fn set_scroll_container_def(&mut self, id: u32, content_w: f64, content_h: f64) {
        self.scroll_container_def = Some((id, content_w, content_h));
    }

    pub fn scroll_container_def(&self) -> Option<(u32, f64, f64)> {
        self.scroll_container_def
    }

    pub fn background_size(&self) -> Option<(u8, f64, bool, f64, bool)> {
        self.background_size
    }

    pub fn custom_properties(&self) -> Option<&CustomProperties> {
        self.custom_properties.as_ref()
    }

    pub fn set_custom_properties(&mut self, props: CustomProperties) {
        self.custom_properties = Some(props);
    }

    pub fn background_no_repeat(&self) -> bool {
        self.background_no_repeat
    }

    pub fn text_align(&self) -> TextAlign {
        self.text_align.unwrap_or(TextAlign::Left)
    }

    pub fn set_text_align(&mut self, text_align: TextAlign) {
        self.text_align = Some(text_align);
    }

    pub fn set_color(&mut self, color: Color) {
        self.color = Some(color);
    }

    pub fn color(&self) -> Color {
        self.color
            .clone()
            .expect("failed to access CSS property: color")
    }

    pub fn set_display(&mut self, display: DisplayType) {
        self.display = Some(display);
    }

    pub fn display(&self) -> DisplayType {
        self.display
            .expect("failed to access CSS property: display")
    }

    pub fn set_flex_direction(&mut self, dir: FlexDirection) {
        self.flex_direction = Some(dir);
    }

    /// Flex main-axis direction; defaults to `row` per CSS Flexbox.
    pub fn flex_direction(&self) -> FlexDirection {
        self.flex_direction.unwrap_or(FlexDirection::Row)
    }

    pub fn set_grid_template_columns(&mut self, tracks: Vec<GridTrack>) {
        if !tracks.is_empty() {
            self.grid_template_columns = Some(tracks);
        }
    }

    /// Column tracks of a grid container; a grid without
    /// `grid-template-columns` is a single auto column per CSS Grid §7.1.
    pub fn grid_template_columns(&self) -> Vec<GridTrack> {
        self.grid_template_columns.clone().unwrap_or_else(|| {
            let mut v = Vec::with_capacity(1);
            v.push(GridTrack::Auto);
            v
        })
    }

    /// Column track count of a grid container.
    pub fn grid_columns(&self) -> usize {
        self.grid_template_columns
            .as_ref()
            .map(|t| t.len().max(1))
            .unwrap_or(1)
    }

    pub fn set_column_gap(&mut self, px: f64) {
        self.column_gap = Some(px.max(0.0));
    }

    pub fn column_gap(&self) -> i64 {
        self.column_gap.unwrap_or(0.0) as i64
    }

    pub fn set_row_gap(&mut self, px: f64) {
        self.row_gap = Some(px.max(0.0));
    }

    pub fn row_gap(&self) -> i64 {
        self.row_gap.unwrap_or(0.0) as i64
    }

    pub fn set_font_family(&mut self, font_family: String) {
        self.font_family = Some(font_family);
    }

    pub fn font_family(&self) -> String {
        self.font_family
            .clone()
            .expect("failed to access CSS property: font-family")
    }

    pub fn set_font_size(&mut self, font_size: FontSize) {
        self.font_size = Some(font_size);
    }

    pub fn font_size(&self) -> FontSize {
        self.font_size
            .expect("failed to access CSS property: font-size")
    }

    pub fn font_size_or_default(&self) -> FontSize {
        self.font_size.unwrap_or(FontSize::Medium)
    }
    pub fn set_text_decoration(&mut self, text_decoration: TextDecoration) {
        self.text_decoration = Some(text_decoration);
    }

    pub fn text_decoration(&self) -> TextDecoration {
        self.text_decoration
            .expect("failed to access CSS property: text-decoration")
    }

    pub fn is_bold(&self) -> bool {
        self.bold.unwrap_or(false)
    }

    pub fn set_bold(&mut self, bold: bool) {
        self.bold = Some(bold);
    }

    pub fn list_style_type(&self) -> ListStyleType {
        self.list_style_type.unwrap_or(ListStyleType::None)
    }
    pub fn set_list_style_type(&mut self, v: ListStyleType) {
        self.list_style_type = Some(v);
    }

    pub fn grid_template_areas(&self) -> Option<Rc<Vec<Vec<String>>>> {
        self.grid_template_areas.clone()
    }
    pub fn set_grid_template_areas(&mut self, rows: Vec<Vec<String>>) {
        self.grid_template_areas = Some(Rc::new(rows));
    }
    pub fn grid_column_line_names(&self) -> Option<Rc<Vec<Vec<String>>>> {
        self.grid_column_line_names.clone()
    }
    pub fn set_grid_column_line_names(&mut self, v: Vec<Vec<String>>) {
        if v.iter().any(|names| !names.is_empty()) {
            self.grid_column_line_names = Some(Rc::new(v));
        }
    }

    pub fn grid_area_name(&self) -> Option<&str> {
        self.grid_area_name.as_deref()
    }
    pub fn set_grid_area_name(&mut self, name: String) {
        self.grid_area_name = Some(name);
    }

    pub fn flex_grow(&self) -> f64 {
        self.flex_grow.unwrap_or(0.0)
    }
    pub fn set_flex_grow(&mut self, v: f64) {
        self.flex_grow = Some(v.max(0.0));
    }
    pub fn flex_shrink(&self) -> f64 {
        self.flex_shrink.unwrap_or(1.0)
    }
    pub fn set_flex_shrink(&mut self, v: f64) {
        self.flex_shrink = Some(v.max(0.0));
    }
    pub fn flex_basis(&self) -> Option<f64> {
        self.flex_basis
    }
    pub fn set_flex_basis(&mut self, v: Option<f64>) {
        self.flex_basis = v;
    }
    pub fn justify_content(&self) -> JustifyContent {
        self.justify_content.unwrap_or(JustifyContent::FlexStart)
    }
    pub fn set_justify_content(&mut self, v: JustifyContent) {
        self.justify_content = Some(v);
    }
    pub fn align_items(&self) -> AlignItems {
        self.align_items.unwrap_or(AlignItems::Stretch)
    }
    pub fn set_align_items(&mut self, v: AlignItems) {
        self.align_items = Some(v);
    }
    /// Effective cross-axis alignment for an item inside `container_align`.
    pub fn align_self_or(&self, container_align: AlignItems) -> AlignItems {
        self.align_self.unwrap_or(container_align)
    }
    pub fn set_align_self(&mut self, v: AlignItems) {
        self.align_self = Some(v);
    }

    /// `box-sizing: border-box` — explicit width/height include padding and
    /// border. Not inherited (pages opt in with `* { box-sizing: border-box }`,
    /// which the universal selector applies per element).
    pub fn is_border_box(&self) -> bool {
        self.box_sizing_border_box.unwrap_or(false)
    }

    pub fn set_border_box(&mut self, v: bool) {
        self.box_sizing_border_box = Some(v);
    }

    pub fn set_min_width(&mut self, v: Option<SizeLimit>) {
        self.min_width = v;
    }
    pub fn set_max_width(&mut self, v: Option<SizeLimit>) {
        self.max_width = v;
    }
    pub fn set_min_height(&mut self, v: Option<SizeLimit>) {
        self.min_height = v;
    }
    pub fn set_max_height(&mut self, v: Option<SizeLimit>) {
        self.max_height = v;
    }

    /// Clamp a used width to min-/max-width (CSS2.2 §10.4; min wins over
    /// max). Percentages resolve against the containing block width; when
    /// that is unknown (<= 0) percentage limits are ignored.
    pub fn clamp_width(&self, width: i64, containing: i64) -> i64 {
        let mut w = width;
        if let Some(px) = self.max_width.as_ref().and_then(|l| l.resolve(containing)) {
            w = w.min(px);
        }
        if let Some(px) = self.min_width.as_ref().and_then(|l| l.resolve(containing)) {
            w = w.max(px);
        }
        w
    }

    /// Clamp a used height to min-/max-height (CSS2.2 §10.7).
    pub fn clamp_height(&self, height: i64, containing: i64) -> i64 {
        let mut h = height;
        if let Some(px) = self.max_height.as_ref().and_then(|l| l.resolve(containing)) {
            h = h.min(px);
        }
        if let Some(px) = self.min_height.as_ref().and_then(|l| l.resolve(containing)) {
            h = h.max(px);
        }
        h
    }

    pub fn has_size_limits(&self) -> bool {
        self.min_width.is_some()
            || self.max_width.is_some()
            || self.min_height.is_some()
            || self.max_height.is_some()
    }

    /// `visibility: hidden` — the box keeps its layout size but paints
    /// nothing (unlike display:none, which removes the box).
    pub fn is_visibility_hidden(&self) -> bool {
        self.visibility_hidden.unwrap_or(false)
    }

    pub fn set_visibility_hidden(&mut self, hidden: bool) {
        self.visibility_hidden = Some(hidden);
    }

    pub fn set_opacity(&mut self, opacity: f64) {
        self.opacity = Some(opacity.clamp(0.0, 1.0));
    }

    pub fn opacity(&self) -> f64 {
        self.opacity
            .expect("failed to access CSS property: opacity")
    }

    pub fn set_height(&mut self, height: f64) {
        self.height = Some(height);
        self.height_author = true;
        self.height_ratio = None;
    }

    pub fn set_height_ratio(&mut self, ratio: f64) {
        self.height_ratio = Some(ratio);
        self.height = Some(0.0);
    }

    /// True when the author wrote a literal `height: 0` (not a percentage
    /// that happened to resolve to 0). Real pages collapse dropdown panels
    /// with `height:0; overflow:hidden`, so zero must not read as "auto".
    pub fn explicit_zero_height(&self) -> bool {
        self.height_author
            && self.height_ratio.is_none()
            && matches!(self.height, Some(h) if h == 0.0)
    }

    /// See `explicit_zero_height`.
    pub fn explicit_zero_width(&self) -> bool {
        self.width_author
            && self.width_ratio.is_none()
            && matches!(self.width, Some(w) if w == 0.0)
    }

    pub fn height(&self) -> f64 {
        self.height.expect("failed to access CSS property: height")
    }

    pub fn height_ratio(&self) -> Option<f64> {
        self.height_ratio
    }

    pub fn set_width(&mut self, width: f64) {
        self.width = Some(width);
        self.width_author = true;
        self.width_ratio = None;
    }

    pub fn set_width_ratio(&mut self, ratio: f64) {
        self.width_ratio = Some(ratio);
        self.width = Some(0.0);
    }

    pub fn width(&self) -> f64 {
        self.width.expect("failed to access CSS property: width")
    }

    pub fn width_ratio(&self) -> Option<f64> {
        self.width_ratio
    }

    pub fn set_margin_all(&mut self, value: f64) {
        self.margin = Some(EdgeSize::all(value));
    }

    pub fn set_margin(&mut self, margin: EdgeSize) {
        self.margin = Some(margin);
    }

    pub fn set_margin_left_auto(&mut self, enabled: bool) {
        self.margin_left_auto = enabled;
    }

    pub fn set_margin_right_auto(&mut self, enabled: bool) {
        self.margin_right_auto = enabled;
    }

    pub fn margin_left_auto(&self) -> bool {
        self.margin_left_auto
    }

    pub fn margin_right_auto(&self) -> bool {
        self.margin_right_auto
    }

    pub fn margin_horizontal_auto(&self) -> bool {
        self.margin_left_auto && self.margin_right_auto
    }

    pub fn margin(&self) -> EdgeSize {
        self.margin.expect("failed to access CSS property: margin")
    }

    /// Returns computed margin if already cascaded/defaulted, otherwise CSS initial value (0).
    /// Spec: CSS2.2 margin initial value is `0`.
    /// https://www.w3.org/TR/CSS22/box.html#margin-properties
    pub fn margin_or_default(&self) -> EdgeSize {
        self.margin.unwrap_or(EdgeSize::zero())
    }

    pub fn set_padding_all(&mut self, value: f64) {
        self.padding = Some(EdgeSize::all(value));
    }

    pub fn set_padding(&mut self, padding: EdgeSize) {
        self.padding = Some(padding);
    }

    pub fn padding(&self) -> EdgeSize {
        self.padding
            .expect("failed to access CSS property: padding")
    }

    /// Padding during the cascade (before defaulting fills it): zero when
    /// no earlier declaration set it.
    pub fn padding_or_zero(&self) -> EdgeSize {
        self.padding.unwrap_or_else(EdgeSize::zero)
    }

    pub fn set_border_all(&mut self, value: f64) {
        self.border = Some(EdgeSize::all(value));
    }

    /// Overwrite one border side's width, keeping the others.
    pub fn set_border_side(&mut self, side: usize, px: f64) {
        let b = self.border_or_zero();
        let (mut t, mut r, mut bo, mut l) = (b.top(), b.right(), b.bottom(), b.left());
        match side {
            0 => t = px,
            1 => r = px,
            2 => bo = px,
            _ => l = px,
        }
        self.border = Some(EdgeSize::from_values(t, r, bo, l));
    }

    pub fn set_border(&mut self, border: EdgeSize) {
        self.border = Some(border);
    }

    pub fn border(&self) -> EdgeSize {
        self.border.expect("failed to access CSS property: border")
    }

    /// Returns the border EdgeSize, or `EdgeSize::zero()` if not yet set.
    /// Use this in paint-mapping where `defaulting()` may not have been called.
    pub fn border_or_zero(&self) -> EdgeSize {
        self.border.unwrap_or(EdgeSize::zero())
    }

    pub fn set_border_color(&mut self, color: Color) {
        self.border_color = Some(color);
    }

    pub fn border_color(&self) -> Option<Color> {
        self.border_color.clone()
    }

    pub fn set_position(&mut self, position: PositionType) {
        self.position = Some(position);
    }

    pub fn position(&self) -> PositionType {
        self.position
            .expect("failed to access CSS property: position")
    }

    pub fn position_or_default(&self) -> PositionType {
        self.position.unwrap_or(PositionType::Static)
    }

    pub fn set_float(&mut self, value: Float) {
        self.float = Some(value);
    }

    pub fn float_or_default(&self) -> Float {
        self.float.unwrap_or(Float::None)
    }

    pub fn set_clear(&mut self, value: Clear) {
        self.clear = Some(value);
    }

    pub fn clear_or_default(&self) -> Clear {
        self.clear.unwrap_or(Clear::None)
    }

    pub fn set_transitions(&mut self, transitions: Vec<TransitionSpec>) {
        self.transitions = transitions;
    }

    pub fn transitions(&self) -> &[TransitionSpec] {
        &self.transitions
    }

    /// The transition covering `property` (or the `all` catch-all), if any.
    pub fn transition_for(&self, property: &str) -> Option<&TransitionSpec> {
        self.transitions
            .iter()
            .find(|t| t.property == property || t.property == "all")
    }

    pub fn offset_top_author(&self) -> bool {
        self.offset_top_author
    }

    pub fn offset_left_author(&self) -> bool {
        self.offset_left_author
    }

    pub fn set_offset_top(&mut self, top: f64) {
        self.offset_top_author = true;
        self.offset_top = Some(top);
    }

    pub fn offset_top(&self) -> f64 {
        self.offset_top.expect("failed to access CSS property: top")
    }

    pub fn set_offset_left(&mut self, left: f64) {
        self.offset_left_author = true;
        self.offset_left = Some(left);
    }

    pub fn offset_left(&self) -> f64 {
        self.offset_left
            .expect("failed to access CSS property: left")
    }

    pub fn set_offset_top_ratio(&mut self, r: f64) {
        self.offset_top_author = true;
        self.offset_top_ratio = Some(r);
    }

    pub fn offset_top_ratio(&self) -> Option<f64> {
        self.offset_top_ratio
    }

    pub fn set_offset_left_ratio(&mut self, r: f64) {
        self.offset_left_author = true;
        self.offset_left_ratio = Some(r);
    }

    pub fn offset_left_ratio(&self) -> Option<f64> {
        self.offset_left_ratio
    }

    pub fn set_offset_right(&mut self, right: f64) {
        self.offset_right = Some(right);
    }

    /// `right`, when declared (no defaulting — None means "not specified").
    pub fn offset_right(&self) -> Option<f64> {
        self.offset_right
    }

    pub fn set_offset_bottom(&mut self, bottom: f64) {
        self.offset_bottom = Some(bottom);
    }

    /// `bottom`, when declared (no defaulting — None means "not specified").
    pub fn offset_bottom(&self) -> Option<f64> {
        self.offset_bottom
    }

    pub fn set_z_index(&mut self, z_index: i32) {
        self.z_index = Some(z_index);
    }

    pub fn z_index(&self) -> i32 {
        self.z_index
            .expect("failed to access CSS property: z-index")
    }

    pub fn set_overflow_clip(&mut self, clip: bool) {
        self.overflow_clip = Some(clip);
    }

    pub fn overflow_clip(&self) -> bool {
        self.overflow_clip
            .expect("failed to access CSS property: overflow")
    }
}

/// A min-/max-width/height constraint: absolute px or a fraction of the
/// containing block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SizeLimit {
    Px(f64),
    Ratio(f64),
}

impl SizeLimit {
    fn resolve(&self, containing: i64) -> Option<i64> {
        match self {
            SizeLimit::Px(v) => Some(*v as i64),
            SizeLimit::Ratio(r) if containing > 0 => Some((containing as f64 * r) as i64),
            SizeLimit::Ratio(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextTransform {
    None,
    Uppercase,
    Lowercase,
    Capitalize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WhiteSpace {
    Normal,
    Nowrap,
    Pre,
    PreWrap,
    PreLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListStyleType {
    None,
    Disc,
    Circle,
    Square,
    Decimal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JustifyContent {
    FlexStart,
    FlexEnd,
    Center,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignItems {
    Stretch,
    FlexStart,
    Center,
    FlexEnd,
    Baseline,
}

/// A parsed `linear-gradient(...)`. `angle_deg` follows the CSS
/// convention: 0deg points up (start color at the bottom), 90deg right,
/// 180deg down. Stops carry their color and position along the line (0..1);
/// positions are filled in evenly when omitted.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearGradient {
    pub angle_deg: f64,
    pub stops: Vec<(Color, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Color {
    name: Option<String>,
    code: String,
}

impl Color {
    /// Build a color from resolved channel values (rgb()/hsl() functions).
    /// Alpha 255 yields a 6-digit code; anything else an 8-digit one, which
    /// the platform painter's hex parser understands.
    pub fn from_rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        let code = if a == 255 {
            format!("#{:02x}{:02x}{:02x}", r, g, b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", r, g, b, a)
        };
        Self { name: None, code }
    }

    pub fn from_name(name: &str) -> Result<Self, Error> {
        let code = match name {
            "black" => "#000000".to_string(),
            "silver" => "#c0c0c0".to_string(),
            "gray" => "#808080".to_string(),
            "white" => "#ffffff".to_string(),
            "maroon" => "#800000".to_string(),
            "red" => "#ff0000".to_string(),
            "purple" => "#800080".to_string(),
            "fuchsia" => "#ff00ff".to_string(),
            "green" => "#008000".to_string(),
            "lime" => "#00ff00".to_string(),
            "olive" => "#808000".to_string(),
            "yellow" => "#ffff00".to_string(),
            "navy" => "#000080".to_string(),
            "blue" => "#0000ff".to_string(),
            "teal" => "#008080".to_string(),
            "aqua" => "#00ffff".to_string(),
            "orange" => "#ffa500".to_string(),
            "lightgray" => "#d3d3d3".to_string(),
            "transparent" => "#00000000".to_string(),
            _ => {
                return Err(Error::UnexpectedInput(format!(
                    "color name {:?} is not supported yet",
                    name
                )));
            }
        };

        Ok(Self {
            name: Some(name.to_string()),
            code,
        })
    }

    pub fn from_code(code: &str) -> Result<Self, Error> {
        if code.chars().nth(0) != Some('#') {
            return Err(Error::UnexpectedInput(format!(
                "invalid color code: {}",
                code
            )));
        }

        // #rgb and #rgba expand each nibble; #rrggbb and #rrggbbaa are kept
        // as-is. The 8-digit (alpha) form flows through unchanged — the
        // platform painter's hex parser reads the trailing alpha byte.
        let normalized = if code.len() == 4 || code.len() == 5 {
            let mut expanded = String::from("#");
            for ch in code.chars().skip(1) {
                expanded.push(ch);
                expanded.push(ch);
            }
            expanded
        } else {
            code.to_string()
        };

        if normalized.len() != 7 && normalized.len() != 9 {
            return Err(Error::UnexpectedInput(format!(
                "invalid color code: {}",
                code
            )));
        }

        if normalized.chars().skip(1).any(|ch| !ch.is_ascii_hexdigit()) {
            return Err(Error::UnexpectedInput(format!(
                "invalid color code: {}",
                code
            )));
        }

        let name = match normalized.as_str() {
            "#000000" => Some("black".to_string()),
            "#c0c0c0" => Some("silver".to_string()),
            "#808080" => Some("gray".to_string()),
            "#ffffff" => Some("white".to_string()),
            "#800000" => Some("maroon".to_string()),
            "#ff0000" => Some("red".to_string()),
            "#800080" => Some("purple".to_string()),
            "#ff00ff" => Some("fuchsia".to_string()),
            "#008000" => Some("green".to_string()),
            "#00ff00" => Some("lime".to_string()),
            "#808000" => Some("olive".to_string()),
            "#ffff00" => Some("yellow".to_string()),
            "#000080" => Some("navy".to_string()),
            "#0000ff" => Some("blue".to_string()),
            "#008080" => Some("teal".to_string()),
            "#00ffff" => Some("aqua".to_string()),
            "#ffa500" => Some("orange".to_string()),
            "#d3d3d3" => Some("lightgray".to_string()),
            _ => None,
        };

        Ok(Self {
            name,
            code: normalized,
        })
    }

    pub fn white() -> Self {
        Self {
            name: Some("white".to_string()),
            code: "#ffffff".to_string(),
        }
    }

    pub fn transparent() -> Self {
        Self {
            name: Some("transparent".to_string()),
            code: "#00000000".to_string(),
        }
    }

    pub fn black() -> Self {
        Self {
            name: Some("black".to_string()),
            code: "#000000".to_string(),
        }
    }

    pub fn link_blue() -> Self {
        Self {
            name: Some("blue".to_string()),
            code: "#0000ee".to_string(),
        }
    }

    pub fn gray() -> Self {
        Self {
            name: Some("gray".to_string()),
            code: "#808080".to_string(),
        }
    }

    pub fn lightgray() -> Self {
        Self {
            name: Some("lightgray".to_string()),
            code: "#d3d3d3".to_string(),
        }
    }

    /// Straight-alpha channels from the hex code (alpha 255 when the code has
    /// no alpha byte). Used to interpolate colors during transitions.
    pub fn rgba_channels(&self) -> (u8, u8, u8, u8) {
        let hex = self.code.trim_start_matches('#');
        let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0);
        if hex.len() >= 6 {
            (
                byte(0),
                byte(2),
                byte(4),
                if hex.len() >= 8 { byte(6) } else { 255 },
            )
        } else {
            (0, 0, 0, 255)
        }
    }

    pub fn code_u32(&self) -> u32 {
        u32::from_str_radix(self.code.trim_start_matches('#'), 16).unwrap()
    }

    pub fn code(&self) -> &str {
        &self.code
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum FontSize {
    Medium,
    XLarge,
    XXLarge,
    /// Arbitrary pixel size resolved from a CSS length (e.g. 10pt → 13px).
    /// The legacy named buckets are kept for element defaults (h1–h3) and
    /// keyword values; CSS numeric lengths use this variant so that sizes
    /// like 7pt/13px actually render smaller than the 16px default.
    Px(i64),
}

impl FontSize {
    fn default(node: &Rc<RefCell<Node>>) -> Self {
        match &node.borrow().kind() {
            NodeKind::Element(element) => match element.kind() {
                ElementKind::H1 => FontSize::XXLarge,
                ElementKind::H2 => FontSize::XLarge,
                ElementKind::H3 => FontSize::XLarge,
                _ => FontSize::Medium,
            },
            _ => FontSize::Medium,
        }
    }

    pub fn from_str(value: &str) -> Result<Self, Error> {
        match value {
            "medium" => Ok(Self::Medium),
            "large" | "x-large" => Ok(Self::XLarge),
            "xx-large" => Ok(Self::XXLarge),
            _ => Err(Error::UnexpectedInput(format!(
                "font-size {:?} is not supported yet",
                value
            ))),
        }
    }

    pub fn from_px(value: f64) -> Self {
        // Clamp to a sane range: tiny fonts stay legible (and avoid zero/negative
        // sizes), huge fonts don't blow up layout estimates.
        Self::Px((value.round() as i64).clamp(6, 128))
    }

    pub fn px(&self) -> i64 {
        match self {
            FontSize::Medium => 16,
            FontSize::XLarge => 24,
            FontSize::XXLarge => 32,
            FontSize::Px(n) => *n,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum PositionType {
    Static,
    Relative,
    Absolute,
    /// Anchored to the viewport via top/left and exempt from scrolling.
    Fixed,
    /// Normal flow until its box would scroll past the `top` threshold, then
    /// pinned there (painter-side clamping via the sticky context).
    Sticky,
}

impl PositionType {
    pub fn from_str(value: &str) -> Result<Self, Error> {
        match value {
            "static" => Ok(Self::Static),
            "relative" => Ok(Self::Relative),
            "absolute" => Ok(Self::Absolute),
            "fixed" => Ok(Self::Fixed),
            "sticky" => Ok(Self::Sticky),
            _ => Err(Error::UnexpectedInput(format!(
                "position {:?} is not supported yet",
                value
            ))),
        }
    }
}

/// Attribute the runtime's transition driver writes the interpolated opacity
/// to. It lives on the DOM (rather than in the cascade) so the declared value
/// stays readable as the transition's *target*, and so a full re-layout
/// reproduces the animated frame exactly (the `COSMO_LAYOUT_ASSERT` safety net
/// keeps holding during animations).
pub const ANIM_OPACITY_ATTR: &str = "data-cosmo-anim-opacity";
/// As [`ANIM_OPACITY_ATTR`], for `background-color` (a hex color code).
pub const ANIM_BACKGROUND_COLOR_ATTR: &str = "data-cosmo-anim-background-color";
/// As [`ANIM_OPACITY_ATTR`], for `color` (a hex color code).
pub const ANIM_COLOR_ATTR: &str = "data-cosmo-anim-color";
/// As [`ANIM_OPACITY_ATTR`], for `width` / `height` (pixel lengths).
pub const ANIM_WIDTH_ATTR: &str = "data-cosmo-anim-width";
pub const ANIM_HEIGHT_ATTR: &str = "data-cosmo-anim-height";

/// A property the transition driver knows how to interpolate. Adding one means
/// giving it an override attribute, a target reader
/// ([`ComputedStyle::animated_target`]) and a `used_*` accessor that painting
/// reads instead of the cascade value.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum AnimatedProperty {
    Opacity,
    BackgroundColor,
    Color,
    Width,
    Height,
}

impl AnimatedProperty {
    /// Every property the driver can animate — iterated when collecting targets.
    pub const ALL: [Self; 5] = [
        Self::Opacity,
        Self::BackgroundColor,
        Self::Color,
        Self::Width,
        Self::Height,
    ];

    /// The CSS property name, as it appears in a `transition` declaration.
    pub fn css_name(&self) -> &'static str {
        match self {
            Self::Opacity => "opacity",
            Self::BackgroundColor => "background-color",
            Self::Color => "color",
            Self::Width => "width",
            Self::Height => "height",
        }
    }

    /// The DOM attribute the driver parks the interpolated value in.
    pub fn attr_name(&self) -> &'static str {
        match self {
            Self::Opacity => ANIM_OPACITY_ATTR,
            Self::BackgroundColor => ANIM_BACKGROUND_COLOR_ATTR,
            Self::Color => ANIM_COLOR_ATTR,
            Self::Width => ANIM_WIDTH_ATTR,
            Self::Height => ANIM_HEIGHT_ATTR,
        }
    }
}

/// The value of an animatable property, reduced to interpolatable components.
#[derive(Debug, Clone, PartialEq)]
pub enum AnimatedValue {
    Number(f64),
    /// Straight-alpha RGBA channels.
    Rgba(u8, u8, u8, u8),
}

impl AnimatedValue {
    /// Linear interpolation towards `to` at eased progress `t` in [0,1].
    /// Mismatched shapes (which the driver never produces) snap to `to`.
    /// Spec: CSS Transitions L1 §5 — per-component interpolation; colors
    /// interpolate in premultiplied-free sRGB here (close enough visually).
    pub fn lerp(&self, to: &Self, t: f64) -> Self {
        match (self, to) {
            (Self::Number(a), Self::Number(b)) => Self::Number(a + (b - a) * t),
            (Self::Rgba(ar, ag, ab, aa), Self::Rgba(br, bg, bb, ba)) => {
                let mix = |a: u8, b: u8| (a as f64 + (b as f64 - a as f64) * t).round() as u8;
                Self::Rgba(mix(*ar, *br), mix(*ag, *bg), mix(*ab, *bb), mix(*aa, *ba))
            }
            _ => to.clone(),
        }
    }

    /// Serialized form written to the override attribute.
    pub fn to_attr_value(&self) -> String {
        match self {
            Self::Number(v) => format!("{:.4}", v),
            Self::Rgba(r, g, b, a) => Color::from_rgba(*r, *g, *b, *a).code().to_string(),
        }
    }

    /// Whether two values are close enough to treat as the same target (and
    /// not worth a repaint).
    pub fn approx_eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Number(a), Self::Number(b)) => (a - b).abs() <= 0.0005,
            (a, b) => a == b,
        }
    }
}

/// A single `transition` declaration (one property). Spec: CSS Transitions L1.
#[derive(Debug, Clone, PartialEq)]
pub struct TransitionSpec {
    /// Transitioned property name (e.g. "opacity", "color", "all").
    pub property: String,
    pub duration_ms: u32,
    pub delay_ms: u32,
    pub easing: Easing,
}

/// Timing function for transitions/animations (subset).
#[derive(Debug, Copy, Clone, PartialEq)]
pub enum Easing {
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl Easing {
    pub fn from_str(v: &str) -> Self {
        match v {
            "linear" => Self::Linear,
            "ease-in" => Self::EaseIn,
            "ease-out" => Self::EaseOut,
            "ease-in-out" => Self::EaseInOut,
            _ => Self::Ease,
        }
    }

    /// Map linear progress `t` in [0,1] to eased progress. Cubic-bezier curves
    /// are approximated with smoothstep-family shapes (visually close enough).
    pub fn apply(&self, t: f64) -> f64 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Self::Linear => t,
            Self::EaseIn => t * t,
            Self::EaseOut => t * (2.0 - t),
            // ease / ease-in-out: smoothstep.
            Self::Ease | Self::EaseInOut => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// CSS `float` — takes the box out of normal flow and shifts it to the left or
/// right edge of its containing block; subsequent content flows around it.
/// Spec: CSS2.2 §9.5. https://www.w3.org/TR/CSS22/visuren.html#floats
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Float {
    None,
    Left,
    Right,
}

impl Float {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            _ => None,
        }
    }
}

/// CSS `clear` — moves the box below any preceding left/right (or both) floats.
/// Spec: CSS2.2 §9.5.2.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Clear {
    None,
    Left,
    Right,
    Both,
}

impl Clear {
    pub fn from_str(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "both" => Some(Self::Both),
            _ => None,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum DisplayType {
    Block,
    Inline,
    /// `display:flex` — a block-level flex container. Its own box participates
    /// in normal flow like a block; its children are laid out along the flex
    /// main axis (see [`FlexDirection`]).
    Flex,
    /// `display:grid` — a block-level grid container. Children are placed
    /// row-major into the equal-width column tracks declared by
    /// `grid-template-columns` (track count only; no named lines/areas).
    Grid,
    /// `display:inline-block` — an atomic box that flows inline but
    /// shrink-wraps its content like a block (explicit width/height honored).
    InlineBlock,
    /// `display:contents` — the element generates no box of its own.
    /// Approximated as a zero-decoration full-width block whose children
    /// resolve their grid/flex placement against the nearest non-contents
    /// ancestor.
    Contents,
    DisplayNone,
}

/// Main-axis direction of a flex container.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum FlexDirection {
    Row,
    Column,
}

impl FlexDirection {
    pub fn from_str(value: &str) -> Self {
        match value.trim() {
            "column" | "column-reverse" => FlexDirection::Column,
            // "row", "row-reverse", and anything unknown default to row.
            _ => FlexDirection::Row,
        }
    }
}

impl DisplayType {
    fn default(node: &Rc<RefCell<Node>>) -> Self {
        match &node.borrow().kind() {
            NodeKind::Document => DisplayType::Block,
            NodeKind::Element(e) => {
                if e.is_non_rendered_element() {
                    DisplayType::DisplayNone
                } else if e.is_block_element() {
                    DisplayType::Block
                } else {
                    DisplayType::Inline
                }
            }
            NodeKind::Text(_) => DisplayType::Inline,
        }
    }

    pub fn from_str(s: &str) -> Result<Self, Error> {
        // Map the outer display keyword. The engine has no flex/grid formatting
        // context, so `flex`/`grid`/`table`/`flow-root`/unknown values are
        // approximated as block flow. Crucially they must NOT fall through to
        // `display:none`: real pages set their main containers to `flex`/`grid`,
        // and hiding those blanks the entire page.
        match s.trim() {
            "none" => Ok(Self::DisplayNone),
            // Flex containers get real (if basic) flex layout. inline-flex is
            // treated as a block-level flex container for simplicity.
            "flex" | "inline-flex" => Ok(Self::Flex),
            // Grid containers get basic row-major track placement.
            "grid" => Ok(Self::Grid),
            "inline-block" => Ok(Self::InlineBlock),
            "inline" | "inline-grid" | "inline-table" => Ok(Self::Inline),
            "contents" => Ok(Self::Contents),
            _ => Ok(Self::Block),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum TextDecoration {
    None,
    Underline,
}

impl TextDecoration {
    fn default(node: &Rc<RefCell<Node>>) -> Self {
        match &node.borrow().kind() {
            NodeKind::Element(element) => match element.kind() {
                ElementKind::A => TextDecoration::Underline,
                _ => TextDecoration::None,
            },
            _ => TextDecoration::None,
        }
    }

    pub fn from_str(value: &str) -> Result<Self, Error> {
        match value {
            "none" => Ok(Self::None),
            "underline" => Ok(Self::Underline),
            _ => Err(Error::UnexpectedInput(format!(
                "text-decoration {:?} is not supported yet",
                value
            ))),
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub enum TextAlign {
    Left,
    Center,
    Right,
}

fn get_element_attribute(node: &Rc<RefCell<Node>>, name: &str) -> Option<String> {
    match node.borrow().kind() {
        NodeKind::Element(ref element) => element.get_attribute(name),
        _ => None,
    }
}

/// Walk up the DOM tree to find the nearest ancestor `<table>` and return
/// the value of its `border` attribute (0 if absent or zero).
/// Used to propagate `<TABLE BORDER=N>` to individual `<td>`/`<th>` cells.
fn find_ancestor_table_border(node: &Rc<RefCell<Node>>) -> i64 {
    let mut p = node.borrow().parent().upgrade();
    loop {
        let Some(current) = p else { return 0 };
        if current.borrow().element_kind() == Some(ElementKind::Table) {
            let border_str = get_element_attribute(&current, "border");
            return border_str
                .map(|s| {
                    if s.trim().is_empty() {
                        1 // bare `border` attribute with no value → 1
                    } else {
                        s.trim().parse::<i64>().unwrap_or(0)
                    }
                })
                .unwrap_or(0);
        }
        let next = current.borrow().parent().upgrade();
        p = next;
    }
}

/// Walk up the DOM tree to find the nearest ancestor `<table>` and return
/// the value of its `cellspacing` attribute (default 2 per HTML4).
/// Used to add gaps between table rows so adjacent cell borders don't merge.
/// Returns 0 when no TABLE ancestor is found (e.g., malformed HTML).
///
/// Spec: HTML4.01 §11.3.3 — cellspacing default is 2.
/// https://www.w3.org/TR/html4/struct/tables.html#adef-cellspacing
fn find_ancestor_table_cellspacing(node: &Rc<RefCell<Node>>) -> i64 {
    let mut p = node.borrow().parent().upgrade();
    loop {
        let Some(current) = p else { return 0 };
        if current.borrow().element_kind() == Some(ElementKind::Table) {
            let cs_str = get_element_attribute(&current, "cellspacing");
            return cs_str
                .map(|s| s.trim().parse::<i64>().unwrap_or(2))
                .unwrap_or(2); // HTML4 default
        }
        let next = current.borrow().parent().upgrade();
        p = next;
    }
}

fn parse_html_color(value: &str) -> Option<Color> {
    let trimmed = value.trim();
    if trimmed.starts_with('#') {
        Color::from_code(trimmed).ok()
    } else {
        Color::from_name(trimmed).ok().or_else(|| {
            // Try as bare hex code (e.g., "d2b48c" without #).
            if trimmed.len() == 6 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
                Color::from_code(&format!("#{}", trimmed)).ok()
            } else {
                None
            }
        })
    }
}
