use crate::alloc::string::ToString;
use crate::renderer::css::token::CssToken;
use crate::renderer::css::token::CssTokenizer;
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

    /// https://www.w3.org/TR/css-syntax-3/#consume-component-value
    fn consume_component_value(&mut self) -> Option<ComponentValue> {
        self.t.next()
    }

    fn consume_component_values(&mut self) -> Vec<ComponentValue> {
        let mut values = Vec::new();

        loop {
            match self.t.peek() {
                Some(CssToken::SemiColon) | Some(CssToken::CloseCurly) | None => return values,
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
        match self.t.next() {
            Some(CssToken::Colon) => {}
            Some(_) | None => return None,
        }

        declaration.set_values(self.consume_component_values());

        Some(declaration)
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

    fn consume_selector(&mut self) -> Selector {
        let token = match self.t.next() {
            Some(t) => t,
            // EOF — return an UnknownSelector so the caller can drop the rule.
            None => return Selector::UnknownSelector,
        };

        match token {
            CssToken::HashToken(value) => Selector::IdSelector(value[1..].to_string()),
            CssToken::Delim(delim) => {
                if delim == '.' {
                    return Selector::ClassSelector(self.consume_ident());
                }
                // Other delim characters (`>`, `+`, `~`, `*`, `!`, etc.):
                // treat as an unknown selector rather than crashing.
                Selector::UnknownSelector
            }
            CssToken::Ident(ident) => {
                if self.t.peek() == Some(&CssToken::Colon) {
                    // Skip a pseudo-class/element tail up to the rule block.
                    // MUST also stop at EOF: otherwise `peek()` returning `None`
                    // (which is `!= Some(OpenCurly)`) spins forever on truncated
                    // or unsupported CSS and hangs the renderer.
                    while !matches!(self.t.peek(), Some(&CssToken::OpenCurly) | None) {
                        self.t.next();
                    }
                }
                Selector::TypeSelector(ident.to_string())
            }
            CssToken::AtKeyword(_keyword) => {
                // Skip an at-rule prelude (e.g. `@media ...`) up to its block.
                // Stop at EOF as well to avoid an infinite loop — real pages are
                // full of `@media`/`@font-face`/`@supports` at-rules.
                while !matches!(self.t.peek(), Some(&CssToken::OpenCurly) | None) {
                    self.t.next();
                }
                Selector::UnknownSelector
            }
            _ => {
                self.t.next();
                Selector::UnknownSelector
            }
        }
    }

    fn consume_qualified_rule(&mut self) -> Option<QualifiedRule> {
        let mut rule = QualifiedRule::new();

        loop {
            let token = match self.t.peek() {
                Some(t) => t,
                None => return None,
            };

            match token {
                CssToken::OpenCurly => {
                    assert_eq!(self.t.next(), Some(CssToken::OpenCurly));
                    rule.set_declarations(self.consume_list_of_declarations());
                    return Some(rule);
                }
                _ => {
                    rule.set_selector(self.consume_selector());
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
        sheet.set_rules(self.consume_list_of_rules());
        sheet
    }
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
    // 1. Collect raw custom properties (last definition wins).
    let mut vars: BTreeMap<String, Vec<CssToken>> = BTreeMap::new();
    for rule in &sheet.rules {
        for decl in &rule.declarations {
            if decl.property.starts_with("--") {
                vars.insert(decl.property.clone(), decl.value.clone());
            }
        }
    }
    if vars.is_empty() {
        return sheet;
    }
    // 2. Resolve var() within custom-property values (nested vars), to a
    //    fixpoint with a small cap to avoid cycles spinning.
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
    // 3. Substitute var() in every non-custom declaration value.
    for rule in &mut sheet.rules {
        for decl in &mut rule.declarations {
            if !decl.property.starts_with("--") && value_has_var(&decl.value) {
                decl.value = substitute_vars(&decl.value, &vars);
            }
        }
    }
    sheet
}

fn value_has_var(value: &[CssToken]) -> bool {
    value
        .iter()
        .any(|t| matches!(t, CssToken::Ident(s) if s == "var"))
}

/// Replace `var(--name[, fallback])` sequences in `value` using `vars`.
/// Unknown variables fall back to the (optional) fallback tokens.
fn substitute_vars(value: &[CssToken], vars: &BTreeMap<String, Vec<CssToken>>) -> Vec<CssToken> {
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
}

#[derive(Debug, Clone, PartialEq)]
pub struct Declaration {
    pub property: String,
    pub value: Vec<ComponentValue>,
}

impl Declaration {
    pub fn new() -> Self {
        Self {
            property: String::new(),
            value: Vec::new(),
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
