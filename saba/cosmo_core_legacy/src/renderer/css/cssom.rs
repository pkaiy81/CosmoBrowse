use crate::alloc::string::ToString;
use crate::renderer::css::token::CssToken;
use crate::renderer::css::token::CssTokenizer;
use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use core::iter::Peekable;

#[derive(Debug, Clone)]
pub struct CssParser {
    t: Peekable<CssTokenizer>,
}

impl CssParser {
    pub fn new(t: CssTokenizer) -> Self {
        Self { t: t.peekable() }
    }

    /// Skip any whitespace tokens. Whitespace is only meaningful inside
    /// selectors (descendant combinator); every other context ignores it.
    fn skip_whitespace(&mut self) {
        while self.t.peek() == Some(&CssToken::Whitespace) {
            self.t.next();
        }
    }

    /// https://www.w3.org/TR/css-syntax-3/#consume-component-value
    fn consume_component_value(&mut self) -> Option<ComponentValue> {
        self.t.next()
    }

    fn consume_component_values(&mut self) -> Vec<ComponentValue> {
        let mut values = Vec::new();

        loop {
            match self.t.peek() {
                Some(CssToken::SemiColon) | Some(CssToken::CloseCurly) | None => return values,
                // Declaration values are token lists without whitespace; the
                // value consumers (shorthands, url(), …) index adjacent tokens.
                Some(CssToken::Whitespace) => {
                    self.t.next();
                }
                _ => match self.consume_component_value() {
                    Some(v) => values.push(v),
                    None => return values,
                },
            }
        }
    }

    fn consume_ident(&mut self) -> String {
        // Return empty on EOF/unexpected token instead of panicking — Wix/CMS
        // pages routinely ship CSS with constructs this engine cannot parse,
        // and crashing the renderer for a stray token is worse than producing
        // an empty declaration that the cascade will harmlessly ignore.
        let token = match self.t.next() {
            Some(t) => t,
            None => return String::new(),
        };

        match token {
            CssToken::Ident(ref ident) => ident.to_string(),
            _ => String::new(),
        }
    }

    /// https://www.w3.org/TR/css-syntax-3/#consume-a-declaration
    fn consume_declaration(&mut self) -> Option<Declaration> {
        if self.t.peek().is_none() {
            return None;
        }

        let mut declaration = Declaration::new();
        declaration.set_property(self.consume_ident());
        self.skip_whitespace();
        match self.t.next() {
            Some(CssToken::Colon) => {}
            Some(_) | None => return None,
        }
        self.skip_whitespace();

        let mut values = self.consume_component_values();
        // Trailing `!important` is a cascade flag, not part of the value.
        // (Whitespace tokens are already filtered from values, so the two
        // tokens are adjacent even for `! important`.)
        // https://www.w3.org/TR/css-cascade-4/#importance
        if values.len() >= 2
            && values[values.len() - 2] == CssToken::Delim('!')
            && matches!(&values[values.len() - 1],
                CssToken::Ident(s) if s.eq_ignore_ascii_case("important"))
        {
            values.truncate(values.len() - 2);
            declaration.important = true;
        }
        declaration.set_values(values);

        Some(declaration)
    }

    /// Parse a freestanding declaration list — the contents of an inline
    /// `style="..."` attribute. Spec: CSS Style Attributes
    /// https://www.w3.org/TR/css-style-attr/#syntax
    pub fn parse_declaration_list(&mut self) -> Vec<Declaration> {
        self.consume_list_of_declarations()
    }

    /// https://www.w3.org/TR/css-syntax-3/#consume-a-list-of-declarations
    fn consume_list_of_declarations(&mut self) -> Vec<Declaration> {
        let mut declarations = Vec::new();

        loop {
            let token = match self.t.peek() {
                Some(t) => t,
                None => return declarations,
            };

            match token {
                CssToken::CloseCurly => {
                    assert_eq!(self.t.next(), Some(CssToken::CloseCurly));
                    return declarations;
                }
                CssToken::SemiColon => {
                    assert_eq!(self.t.next(), Some(CssToken::SemiColon));
                }
                CssToken::Ident(_) => {
                    if let Some(declaration) = self.consume_declaration() {
                        declarations.push(declaration);
                    }
                }
                _ => {
                    self.t.next();
                }
            }
        }
    }

    /// Consume one compound selector: a run of simple selectors with no
    /// separator between them (`div.card#main:hover`). Returns the matching
    /// model: a single simple selector, `Compound` for several, `Never` when
    /// an interaction pseudo-class (`:hover` …) or pseudo-element
    /// (`::before` …) makes the rule inapplicable to static layout.
    /// https://www.w3.org/TR/selectors-4/#compound
    fn consume_compound_selector(&mut self) -> Selector {
        let mut parts: Vec<Selector> = Vec::new();
        let mut never = false;
        let mut saw_universal = false;
        loop {
            match self.t.peek() {
                Some(CssToken::Ident(ident)) => {
                    let ident = ident.clone();
                    self.t.next();
                    parts.push(Selector::TypeSelector(ident));
                }
                Some(CssToken::HashToken(value)) => {
                    let id = value[1..].to_string();
                    self.t.next();
                    parts.push(Selector::IdSelector(id));
                }
                Some(CssToken::Delim('.')) => {
                    self.t.next();
                    let class = self.consume_ident();
                    if class.is_empty() {
                        never = true;
                    } else {
                        parts.push(Selector::ClassSelector(class));
                    }
                }
                Some(CssToken::Delim('*')) => {
                    self.t.next();
                    saw_universal = true;
                }
                Some(CssToken::Delim('[')) => {
                    self.t.next();
                    match self.consume_attribute_selector() {
                        Some(sel) => parts.push(sel),
                        None => never = true,
                    }
                }
                Some(CssToken::Colon) => {
                    // Pseudo-class or (with a second colon) pseudo-element.
                    self.t.next();
                    let pseudo_element = if self.t.peek() == Some(&CssToken::Colon) {
                        self.t.next();
                        true
                    } else {
                        false
                    };
                    let name = self.consume_ident().to_lowercase();
                    // Functional pseudo (`:not(...)`, `:nth-child(2n+1)`):
                    // capture the argument tokens (balanced parens).
                    let mut args: Vec<CssToken> = Vec::new();
                    if self.t.peek() == Some(&CssToken::OpenParenthesis) {
                        self.t.next();
                        let mut depth = 1;
                        while depth > 0 {
                            match self.t.next() {
                                None => break,
                                Some(CssToken::OpenParenthesis) => {
                                    depth += 1;
                                    args.push(CssToken::OpenParenthesis);
                                }
                                Some(CssToken::CloseParenthesis) => {
                                    depth -= 1;
                                    if depth > 0 {
                                        args.push(CssToken::CloseParenthesis);
                                    }
                                }
                                Some(t) => args.push(t),
                            }
                        }
                    }
                    // Interaction pseudo-classes never match in static
                    // rendering; pseudo-elements (generated content like
                    // ::before/::after) have no box in this engine — applying
                    // their declarations to the element itself would leak
                    // decoration styles. Legacy single-colon spellings of
                    // before/after count as pseudo-elements too.
                    let interactive = matches!(
                        name.as_str(),
                        "hover" | "active" | "focus" | "focus-within" | "focus-visible"
                    );
                    let legacy_pseudo_element =
                        matches!(name.as_str(), "before" | "after" | "selection" | "placeholder");
                    if interactive || pseudo_element || legacy_pseudo_element {
                        never = true;
                    } else {
                        match name.as_str() {
                            "root" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::Root))
                            }
                            "first-child" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::FirstChild))
                            }
                            "last-child" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::LastChild))
                            }
                            "only-child" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::OnlyChild))
                            }
                            "nth-child" | "nth-last-child" => match parse_nth_formula(&args) {
                                Some((a, b)) => {
                                    let kind = if name == "nth-child" {
                                        PseudoClassKind::NthChild(a, b)
                                    } else {
                                        PseudoClassKind::NthLastChild(a, b)
                                    };
                                    parts.push(Selector::PseudoClass(kind));
                                }
                                // Unparsable formula: never match rather than
                                // over-match.
                                None => never = true,
                            },
                            // All other pseudo-classes (:link, :visited,
                            // :root, :not(...) …) are approximated as
                            // matching.
                            _ => {}
                        }
                    }
                }
                _ => break,
            }
        }
        if never {
            return Selector::Never;
        }
        match parts.len() {
            0 if saw_universal => Selector::Universal,
            0 => Selector::UnknownSelector,
            1 => parts.pop().expect("len checked"),
            _ => Selector::Compound(parts),
        }
    }

    /// Consume the remainder of an attribute selector after the `[`:
    /// `name ]`, `name = value ]`, or `name ~^$*|= value ]`. Returns None on
    /// malformed input (caller poisons the compound to Never) after skipping
    /// to the closing bracket.
    /// https://www.w3.org/TR/selectors-4/#attribute-selectors
    fn consume_attribute_selector(&mut self) -> Option<Selector> {
        self.skip_whitespace();
        let name = self.consume_ident().to_lowercase();
        self.skip_whitespace();
        let mut result = None;
        if !name.is_empty() {
            match self.t.peek() {
                Some(CssToken::Delim(']')) => {
                    result = Some(Selector::Attribute {
                        name,
                        op: AttrOp::Exists,
                        value: String::new(),
                    });
                }
                Some(CssToken::Delim(c @ ('=' | '~' | '|' | '^' | '$' | '*'))) => {
                    let c = *c;
                    self.t.next();
                    let op = if c == '=' {
                        Some(AttrOp::Equals)
                    } else if self.t.peek() == Some(&CssToken::Delim('=')) {
                        self.t.next();
                        Some(match c {
                            '~' => AttrOp::Includes,
                            '|' => AttrOp::DashMatch,
                            '^' => AttrOp::Prefix,
                            '$' => AttrOp::Suffix,
                            _ => AttrOp::Substring,
                        })
                    } else {
                        None
                    };
                    if let Some(op) = op {
                        self.skip_whitespace();
                        let value = match self.t.peek() {
                            Some(CssToken::StringToken(s)) | Some(CssToken::Ident(s)) => {
                                let v = s.clone();
                                self.t.next();
                                Some(v)
                            }
                            Some(CssToken::Number(n)) => {
                                let v = alloc::format!("{}", n);
                                self.t.next();
                                Some(v)
                            }
                            _ => None,
                        };
                        if let Some(value) = value {
                            result = Some(Selector::Attribute { name, op, value });
                        }
                    }
                }
                _ => {}
            }
        }
        // Skip to (and past) the closing bracket regardless of success; also
        // drop a trailing case-insensitivity flag (`[x=y i]`) on the floor.
        loop {
            match self.t.next() {
                None | Some(CssToken::Delim(']')) => break,
                _ => {}
            }
        }
        result
    }

    /// Consume one complex selector: compound selectors joined by descendant
    /// (whitespace), child (`>`), or sibling (`+`, `~`) combinators, e.g.
    /// `.admin td`, `ul > li`, `h1 + p`.
    /// https://www.w3.org/TR/selectors-4/#complex
    fn consume_complex_selector(&mut self) -> Selector {
        let mut left = self.consume_compound_selector();
        loop {
            let mut saw_ws = false;
            while self.t.peek() == Some(&CssToken::Whitespace) {
                self.t.next();
                saw_ws = true;
            }
            match self.t.peek() {
                Some(CssToken::Delim('>')) => {
                    self.t.next();
                    self.skip_whitespace();
                    let right = self.consume_compound_selector();
                    left = Selector::Child(Box::new(left), Box::new(right));
                }
                Some(CssToken::Delim(c @ ('+' | '~'))) => {
                    let next_sibling = *c == '+';
                    self.t.next();
                    self.skip_whitespace();
                    let right = self.consume_compound_selector();
                    left = if next_sibling {
                        Selector::NextSibling(Box::new(left), Box::new(right))
                    } else {
                        Selector::SubsequentSibling(Box::new(left), Box::new(right))
                    };
                }
                Some(CssToken::Ident(_))
                | Some(CssToken::HashToken(_))
                | Some(CssToken::Delim('.'))
                | Some(CssToken::Delim('*'))
                | Some(CssToken::Delim('['))
                | Some(CssToken::Colon)
                    if saw_ws =>
                {
                    let right = self.consume_compound_selector();
                    left = Selector::Descendant(Box::new(left), Box::new(right));
                }
                _ => return left,
            }
        }
    }

    /// Consume a selector list (`h1, h2 { … }`): complex selectors separated
    /// by commas. https://www.w3.org/TR/selectors-4/#grouping
    fn consume_selector_list(&mut self) -> Selector {
        let mut alternatives = Vec::new();
        loop {
            self.skip_whitespace();
            alternatives.push(self.consume_complex_selector());
            self.skip_whitespace();
            match self.t.peek() {
                Some(CssToken::Delim(',')) => {
                    self.t.next();
                }
                _ => break,
            }
        }
        if alternatives.len() == 1 {
            alternatives.pop().expect("len checked")
        } else {
            Selector::List(alternatives)
        }
    }

    fn consume_qualified_rule(&mut self) -> Option<QualifiedRule> {
        let mut rule = QualifiedRule::new();

        self.skip_whitespace();
        self.t.peek()?;
        rule.set_selector(self.consume_selector_list());

        // Error recovery: skip anything unparseable (attribute selectors,
        // stray delimiters) up to the rule block, poisoning the selector so
        // the partially-understood rule doesn't over-match.
        loop {
            match self.t.peek() {
                None => return None,
                Some(CssToken::OpenCurly) => {
                    assert_eq!(self.t.next(), Some(CssToken::OpenCurly));
                    rule.set_declarations(self.consume_list_of_declarations());
                    return Some(rule);
                }
                Some(_) => {
                    rule.set_selector(Selector::UnknownSelector);
                    self.t.next();
                }
            }
        }
    }

    fn consume_list_of_rules(&mut self) -> Vec<QualifiedRule> {
        let mut rules = Vec::new();

        loop {
            let token = match self.t.peek() {
                Some(t) => t,
                None => return rules,
            };
            match token {
                // Whitespace between rules must be skipped HERE: if it falls
                // through to consume_qualified_rule, an at-rule behind it is
                // never seen by the AtKeyword arm and its block leaks.
                CssToken::Whitespace => {
                    self.t.next();
                }
                CssToken::AtKeyword(_keyword) => {
                    // Skip the whole at-rule. The engine can't evaluate at-rule
                    // preludes (`@media`, `@supports`, `@font-face`, `@import`),
                    // and the previous code parsed the *contents* of an
                    // `@media { ... }` block as top-level rules — producing
                    // spurious rules (e.g. mobile `width`/`display` overrides)
                    // that collapsed desktop layouts such as Hacker News.
                    self.consume_at_rule();
                }
                _ => {
                    let rule = self.consume_qualified_rule();
                    match rule {
                        Some(r) => rules.push(r),
                        None => return rules,
                    }
                }
            }
        }
    }

    /// Consume and discard an at-rule: its prelude up to either a `;`
    /// (statement at-rules like `@import`) or a `{ ... }` block (which is
    /// consumed with balanced braces). Spec: CSS Syntax §5.4.2.
    /// https://www.w3.org/TR/css-syntax-3/#consume-at-rule
    fn consume_at_rule(&mut self) {
        // Consume the at-keyword token itself.
        self.t.next();
        loop {
            match self.t.next() {
                None => return,
                Some(CssToken::SemiColon) => return,
                Some(CssToken::OpenCurly) => {
                    // Discard the block, honouring nested braces.
                    let mut depth = 1;
                    while depth > 0 {
                        match self.t.next() {
                            None => return,
                            Some(CssToken::OpenCurly) => depth += 1,
                            Some(CssToken::CloseCurly) => depth -= 1,
                            _ => {}
                        }
                    }
                    return;
                }
                _ => {}
            }
        }
    }

    pub fn parse_stylesheet(&mut self) -> StyleSheet {
        let mut sheet = StyleSheet::new();
        // Flatten selector lists into one rule per alternative: specificity is
        // a property of the individual complex selector, not the list, so
        // `h1, .x { }` must cascade as a (0,0,1) rule AND a (0,1,0) rule.
        // Document order within the list is preserved.
        let mut rules = Vec::new();
        for rule in self.consume_list_of_rules() {
            match rule.selector {
                Selector::List(alternatives) => {
                    for alt in alternatives {
                        let mut r = QualifiedRule::new();
                        r.set_selector(alt);
                        r.set_declarations(rule.declarations.clone());
                        rules.push(r);
                    }
                }
                _ => rules.push(rule),
            }
        }
        sheet.set_rules(rules);
        sheet
    }
}

/// Parse an `An+B` micro-syntax formula from pseudo-class argument tokens.
/// Returns (A, B). Handles `odd`/`even`, bare integers, and the `An±B`
/// forms in their various tokenizations (`2n+1` → Dimension(2,"n") '+' 1;
/// `2n-1` → Dimension(2,"n-1"); `n`/`-n` → Ident; `-n-3` → Ident("-n-3")).
/// https://www.w3.org/TR/css-syntax-3/#anb-microsyntax
fn parse_nth_formula(tokens: &[CssToken]) -> Option<(i64, i64)> {
    let toks: Vec<&CssToken> = tokens
        .iter()
        .filter(|t| **t != CssToken::Whitespace)
        .collect();
    // Split an ident/unit like "n", "n-3", "-n-3" into (a, optional b).
    fn n_part(s: &str, sign: i64) -> Option<(i64, Option<i64>)> {
        let s = s.to_lowercase();
        let (sign, rest) = if let Some(stripped) = s.strip_prefix('-') {
            (-sign, stripped)
        } else {
            (sign, s.as_str())
        };
        let rest = rest.strip_prefix('n')?;
        if rest.is_empty() {
            return Some((sign, None));
        }
        // Trailing "-3": a negative B fused into the same token.
        let b: i64 = rest.parse().ok()?;
        Some((sign, Some(b)))
    }
    let (a, fused_b, mut i) = match toks.first() {
        Some(CssToken::Ident(s)) if s.eq_ignore_ascii_case("odd") => return Some((2, 1)),
        Some(CssToken::Ident(s)) if s.eq_ignore_ascii_case("even") => return Some((2, 0)),
        Some(CssToken::Number(v)) => return Some((0, *v as i64)),
        Some(CssToken::Dimension(v, unit)) => {
            let (sign, b) = n_part(unit, 1)?;
            ((*v as i64) * sign, b, 1)
        }
        Some(CssToken::Ident(s)) => {
            let (a, b) = n_part(s, 1)?;
            (a, b, 1)
        }
        _ => return None,
    };
    if let Some(b) = fused_b {
        return Some((a, b));
    }
    // Optional `± B` tail.
    let mut b = 0i64;
    if i < toks.len() {
        let sign = match toks.get(i) {
            Some(CssToken::Delim('+')) => 1,
            Some(CssToken::Delim('-')) => -1,
            // Negative numbers tokenize with the sign attached.
            Some(CssToken::Number(v)) => {
                return Some((a, *v as i64));
            }
            _ => return None,
        };
        i += 1;
        match toks.get(i) {
            Some(CssToken::Number(v)) => b = sign * (*v as i64),
            _ => return None,
        }
    }
    Some((a, b))
}

/// Resolve CSS custom properties: collect every `--name: value` declaration
/// into a document-level map, then substitute `var(--name[, fallback])` in all
/// other declaration values. Modern sites place design tokens on `:root` and
/// reference them everywhere; without this their colors/spacing never apply.
///
/// This is a pragmatic, document-global resolution (no per-element cascade of
/// custom properties), which covers the common `:root` design-token pattern.
/// Spec: https://www.w3.org/TR/css-variables-1/
pub fn resolve_css_variables(mut sheet: StyleSheet) -> StyleSheet {
    let vars = collect_custom_properties(&sheet);
    if vars.is_empty() {
        return sheet;
    }
    // Substitute var() in every non-custom declaration value.
    for rule in &mut sheet.rules {
        for decl in &mut rule.declarations {
            if !decl.property.starts_with("--") && value_has_var(&decl.value) {
                decl.value = substitute_vars(&decl.value, &vars);
            }
        }
    }
    sheet
}

/// Collect every `--name: value` declaration in the stylesheet into one map
/// (document order, last definition wins), with nested `var()` references
/// inside custom-property values resolved to a fixpoint. This seeds the
/// document root's custom-property scope; per-element rules then override it
/// during the cascade.
pub fn collect_custom_properties(sheet: &StyleSheet) -> BTreeMap<String, Vec<CssToken>> {
    let mut vars: BTreeMap<String, Vec<CssToken>> = BTreeMap::new();
    for rule in &sheet.rules {
        for decl in &rule.declarations {
            if decl.property.starts_with("--") {
                vars.insert(decl.property.clone(), decl.value.clone());
            }
        }
    }
    // Resolve var() within custom-property values (nested vars), to a
    // fixpoint with a small cap to avoid cycles spinning.
    for _ in 0..5 {
        let snapshot = vars.clone();
        let mut changed = false;
        for value in vars.values_mut() {
            if value_has_var(value) {
                *value = substitute_vars(value, &snapshot);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    vars
}

pub fn value_has_var(value: &[CssToken]) -> bool {
    value
        .iter()
        .any(|t| matches!(t, CssToken::Ident(s) if s == "var"))
}

/// Replace `var(--name[, fallback])` sequences in `value` using `vars`.
/// Unknown variables fall back to the (optional) fallback tokens.
pub fn substitute_vars(value: &[CssToken], vars: &BTreeMap<String, Vec<CssToken>>) -> Vec<CssToken> {
    let mut out: Vec<CssToken> = Vec::new();
    let mut i = 0;
    while i < value.len() {
        let is_var = matches!(&value[i], CssToken::Ident(s) if s == "var")
            && value.get(i + 1) == Some(&CssToken::OpenParenthesis);
        if !is_var {
            out.push(value[i].clone());
            i += 1;
            continue;
        }
        // Find the matching close parenthesis (handle nested parens).
        let mut depth = 1;
        let mut j = i + 2;
        while j < value.len() && depth > 0 {
            match value[j] {
                CssToken::OpenParenthesis => depth += 1,
                CssToken::CloseParenthesis => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            j += 1;
        }
        let inner = &value[(i + 2).min(value.len())..j.min(value.len())];
        let name = match inner.first() {
            Some(CssToken::Ident(n)) => n.clone(),
            _ => String::new(),
        };
        let fallback: Vec<CssToken> = inner
            .iter()
            .position(|t| *t == CssToken::Delim(','))
            .map(|p| inner[p + 1..].to_vec())
            .unwrap_or_default();
        match vars.get(&name) {
            Some(v) => out.extend(v.clone()),
            None => out.extend(fallback),
        }
        i = j + 1; // skip past the close paren
    }
    out
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyleSheet {
    pub rules: Vec<QualifiedRule>,
}

impl StyleSheet {
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    pub fn set_rules(&mut self, rules: Vec<QualifiedRule>) {
        self.rules = rules;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QualifiedRule {
    pub selector: Selector,
    pub declarations: Vec<Declaration>,
}

impl QualifiedRule {
    pub fn new() -> Self {
        Self {
            selector: Selector::TypeSelector("".to_string()),
            declarations: Vec::new(),
        }
    }

    pub fn set_selector(&mut self, selector: Selector) {
        self.selector = selector;
    }

    pub fn set_declarations(&mut self, declarations: Vec<Declaration>) {
        self.declarations = declarations;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selector {
    TypeSelector(String),
    ClassSelector(String),
    IdSelector(String),
    UnknownSelector,
    /// `*` — matches every element.
    Universal,
    /// All parts must match the same element (`div.card#main`).
    Compound(Vec<Selector>),
    /// Right side matches the element, left side matches SOME ancestor
    /// (descendant combinator, `.admin td`).
    Descendant(Box<Selector>, Box<Selector>),
    /// Right side matches the element, left side matches its parent
    /// (child combinator, `ul > li`).
    Child(Box<Selector>, Box<Selector>),
    /// Selector list (`h1, h2`) — any alternative matching suffices.
    List(Vec<Selector>),
    /// Never matches: interaction pseudo-classes (`:hover`), pseudo-elements
    /// (`::before`), and unsupported constructs are poisoned to this rather
    /// than over-matching.
    Never,
    /// Attribute selector `[name]` / `[name<op>value]`.
    /// https://www.w3.org/TR/selectors-4/#attribute-selectors
    Attribute {
        name: String,
        op: AttrOp,
        value: String,
    },
    /// Right side matches the element, left side matches the immediately
    /// preceding sibling element (`a + b`).
    NextSibling(Box<Selector>, Box<Selector>),
    /// Right side matches the element, left side matches SOME preceding
    /// sibling element (`a ~ b`).
    SubsequentSibling(Box<Selector>, Box<Selector>),
    /// Structural pseudo-class testing the element's position among its
    /// element siblings. https://www.w3.org/TR/selectors-4/#structural-pseudos
    PseudoClass(PseudoClassKind),
}

/// Supported structural pseudo-classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PseudoClassKind {
    /// `:root` — the document's root element (`<html>`).
    Root,
    FirstChild,
    LastChild,
    OnlyChild,
    /// `:nth-child(An+B)` — matches the (An+B)-th element child for some
    /// integer n ≥ 0 (1-based). `odd` = 2n+1, `even` = 2n.
    NthChild(i64, i64),
    /// `:nth-last-child(An+B)` — same, counting from the end.
    NthLastChild(i64, i64),
}

/// Attribute selector operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttrOp {
    /// `[name]` — the attribute exists.
    Exists,
    /// `[name=v]` — exact value match.
    Equals,
    /// `[name~=v]` — one of the space-separated words equals v.
    Includes,
    /// `[name|=v]` — value is v or starts with `v-`.
    DashMatch,
    /// `[name^=v]` — value starts with v.
    Prefix,
    /// `[name$=v]` — value ends with v.
    Suffix,
    /// `[name*=v]` — value contains v.
    Substring,
}

impl Selector {
    /// Cascade specificity packed as a single sortable integer:
    /// id count (high byte-pair), then class count, then type count, each
    /// saturated at 255. Combinators sum both sides; `List` is flattened away
    /// at parse time so its value here (max of alternatives) is only a
    /// fallback. Spec: Selectors L4 §17.
    /// https://www.w3.org/TR/selectors-4/#specificity
    pub fn specificity(&self) -> u32 {
        let (a, b, c) = self.specificity_abc();
        (a.min(255) << 16) | (b.min(255) << 8) | c.min(255)
    }

    fn specificity_abc(&self) -> (u32, u32, u32) {
        match self {
            Selector::IdSelector(_) => (1, 0, 0),
            // Attribute selectors and pseudo-classes count like classes.
            // Selectors L4 §17.
            Selector::ClassSelector(_)
            | Selector::Attribute { .. }
            | Selector::PseudoClass(_) => (0, 1, 0),
            Selector::TypeSelector(_) => (0, 0, 1),
            Selector::Universal | Selector::UnknownSelector | Selector::Never => (0, 0, 0),
            Selector::Compound(parts) => parts.iter().fold((0, 0, 0), |acc, p| {
                let (a, b, c) = p.specificity_abc();
                (acc.0 + a, acc.1 + b, acc.2 + c)
            }),
            Selector::Descendant(left, right)
            | Selector::Child(left, right)
            | Selector::NextSibling(left, right)
            | Selector::SubsequentSibling(left, right) => {
                let (la, lb, lc) = left.specificity_abc();
                let (ra, rb, rc) = right.specificity_abc();
                (la + ra, lb + rb, lc + rc)
            }
            Selector::List(alternatives) => alternatives
                .iter()
                .map(|s| s.specificity_abc())
                .max()
                .unwrap_or((0, 0, 0)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub property: String,
    pub value: Vec<ComponentValue>,
    /// `!important` — the declaration outranks every normal declaration in
    /// the cascade, regardless of selector specificity.
    /// https://www.w3.org/TR/css-cascade-4/#importance
    pub important: bool,
}

impl Declaration {
    pub fn new() -> Self {
        Self {
            property: String::new(),
            value: Vec::new(),
            important: false,
        }
    }

    pub fn set_property(&mut self, property: String) {
        self.property = property;
    }

    pub fn set_values(&mut self, value: Vec<ComponentValue>) {
        self.value = value;
    }

    pub fn first_value(&self) -> Option<&ComponentValue> {
        self.value.first()
    }
}

pub type ComponentValue = CssToken;

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    #[test]
    fn test_empty() {
        let style = "".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        assert_eq!(cssom.rules.len(), 0);
    }

    #[test]
    fn test_one_rule() {
        let style = "p { color: red; }".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        let mut rule = QualifiedRule::new();
        rule.set_selector(Selector::TypeSelector("p".to_string()));
        let mut declaration = Declaration::new();
        declaration.set_property("color".to_string());
        declaration.set_values(vec![ComponentValue::Ident("red".to_string())]);
        rule.set_declarations(vec![declaration]);

        let expected = [rule];
        assert_eq!(cssom.rules.len(), expected.len());

        let mut i = 0;
        for rule in &cssom.rules {
            assert_eq!(&expected[i], rule);
            i += 1;
        }
    }

    #[test]
    fn test_id_selector() {
        let style = "#id { color: red; }".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        let mut rule = QualifiedRule::new();
        rule.set_selector(Selector::IdSelector("id".to_string()));
        let mut declaration = Declaration::new();
        declaration.set_property("color".to_string());
        declaration.set_values(vec![ComponentValue::Ident("red".to_string())]);
        rule.set_declarations(vec![declaration]);

        let expected = [rule];
        assert_eq!(cssom.rules.len(), expected.len());

        let mut i = 0;
        for rule in &cssom.rules {
            assert_eq!(&expected[i], rule);
            i += 1;
        }
    }

    #[test]
    fn test_class_selector() {
        let style = ".class { color: red; }".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        let mut rule = QualifiedRule::new();
        rule.set_selector(Selector::ClassSelector("class".to_string()));
        let mut declaration = Declaration::new();
        declaration.set_property("color".to_string());
        declaration.set_values(vec![ComponentValue::Ident("red".to_string())]);
        rule.set_declarations(vec![declaration]);

        let expected = [rule];
        assert_eq!(cssom.rules.len(), expected.len());

        let mut i = 0;
        for rule in &cssom.rules {
            assert_eq!(&expected[i], rule);
            i += 1;
        }
    }

    #[test]
    fn test_multiple_rules() {
        let style = "p { content: \"Hey\"; } h1 { font-size: 40; color: blue; }".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        let mut rule1 = QualifiedRule::new();
        rule1.set_selector(Selector::TypeSelector("p".to_string()));
        let mut declaration1 = Declaration::new();
        declaration1.set_property("content".to_string());
        declaration1.set_values(vec![ComponentValue::StringToken("Hey".to_string())]);
        rule1.set_declarations(vec![declaration1]);

        let mut rule2 = QualifiedRule::new();
        rule2.set_selector(Selector::TypeSelector("h1".to_string()));
        let mut declaration2 = Declaration::new();
        declaration2.set_property("font-size".to_string());
        declaration2.set_values(vec![ComponentValue::Number(40.0)]);
        let mut declaration3 = Declaration::new();
        declaration3.set_property("color".to_string());
        declaration3.set_values(vec![ComponentValue::Ident("blue".to_string())]);
        rule2.set_declarations(vec![declaration2, declaration3]);

        let expected = [rule1, rule2];
        assert_eq!(cssom.rules.len(), expected.len());

        let mut i = 0;
        for rule in &cssom.rules {
            assert_eq!(&expected[i], rule);
            i += 1;
        }
    }

    #[test]
    fn test_multi_value_declaration() {
        let style = "body { margin: 15vh auto; }".to_string();
        let t = CssTokenizer::new(style);
        let cssom = CssParser::new(t).parse_stylesheet();

        assert_eq!(cssom.rules.len(), 1);
        assert_eq!(cssom.rules[0].declarations.len(), 1);
        assert_eq!(
            cssom.rules[0].declarations[0].value,
            vec![
                ComponentValue::Dimension(15.0, "vh".to_string()),
                ComponentValue::Ident("auto".to_string()),
            ]
        );
    }

    fn decl_value<'a>(sheet: &'a StyleSheet, selector: &str, prop: &str) -> &'a [ComponentValue] {
        let rule = sheet
            .rules
            .iter()
            .find(|r| r.selector == Selector::TypeSelector(selector.to_string()))
            .expect("rule");
        &rule
            .declarations
            .iter()
            .find(|d| d.property == prop)
            .expect("decl")
            .value
    }

    #[test]
    fn test_resolves_css_variables_and_fallback() {
        let style = ":root { --brand: #2266cc; } \
            p { color: var(--brand); } \
            h1 { color: var(--missing, #11aa55); }"
            .to_string();
        let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
        let resolved = resolve_css_variables(cssom);
        // Defined variable resolves to its value.
        assert_eq!(
            decl_value(&resolved, "p", "color"),
            &[ComponentValue::HashToken("#2266cc".to_string())]
        );
        // Missing variable falls back to the provided fallback token.
        assert_eq!(
            decl_value(&resolved, "h1", "color"),
            &[ComponentValue::HashToken("#11aa55".to_string())]
        );
    }







    #[test]
    fn test_at_rule_media_block_is_skipped() {
        let style = "td { color: blue; } @media (max-width: 600px) { td { width: 10px; display: block; } } p { color: red; }".to_string();
        let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
        // Only the two top-level rules (td, p) should survive; the @media block
        // and its inner rules must be discarded, not mangled into rules.
        let selectors: alloc::vec::Vec<_> = cssom.rules.iter().map(|r| r.selector.clone()).collect();
        assert_eq!(
            selectors,
            alloc::vec![
                Selector::TypeSelector("td".to_string()),
                Selector::TypeSelector("p".to_string()),
            ],
            "got rules: {:?}", cssom.rules
        );
    }

    #[test]
    fn test_resolves_nested_css_variables() {
        let style = ":root { --base: #abcdef; --accent: var(--base); } \
            div { background: var(--accent); }"
            .to_string();
        let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
        let resolved = resolve_css_variables(cssom);
        assert_eq!(
            decl_value(&resolved, "div", "background"),
            &[ComponentValue::HashToken("#abcdef".to_string())]
        );
    }
}
