use std::collections::HashMap;
use std::string::ToString;
use crate::renderer::css::media::{parse_media_query_list, MediaContext, MediaQueryList};
use crate::renderer::css::token::CssToken;
use crate::renderer::css::token::CssTokenizer;
use std::boxed::Box;
use std::collections::BTreeMap;
use std::string::String;
use std::vec::Vec;
use std::iter::Peekable;
use std::rc::Rc;
use std::vec;

#[derive(Debug, Clone)]
pub struct CssParser {
    t: Peekable<vec::IntoIter<CssToken>>,
    /// What `&` means where this parser is reading: the enclosing rule's
    /// selector inside a nested rule, the subject inside `:has()`.
    nesting: Option<Selector>,
    /// Conditions collected from `@media` preludes; `QualifiedRule::media`
    /// indexes into this.
    media_conditions: Vec<MediaQueryList>,
    /// `@keyframes` rules collected while parsing.
    keyframes: Vec<KeyframesRule>,
}

impl CssParser {
    pub fn new(t: CssTokenizer) -> Self {
        Self::from_tokens(t.collect())
    }

    fn from_tokens(tokens: Vec<CssToken>) -> Self {
        Self {
            t: tokens.into_iter().peekable(),
            nesting: None,
            media_conditions: Vec::new(),
            keyframes: Vec::new(),
        }
    }

    /// Everything up to the `}` closing a block whose `{` was just consumed.
    fn take_block(&mut self) -> Vec<CssToken> {
        let mut depth = 1;
        let mut tokens = Vec::new();
        while let Some(token) = self.t.next() {
            match token {
                CssToken::OpenCurly => depth += 1,
                CssToken::CloseCurly => {
                    depth -= 1;
                    if depth == 0 {
                        return tokens;
                    }
                }
                _ => {}
            }
            tokens.push(token);
        }
        tokens
    }

    /// The tokens of a parenthesized argument whose `(` is next.
    fn take_parenthesized(&mut self) -> Vec<CssToken> {
        let mut tokens = Vec::new();
        if self.t.peek() != Some(&CssToken::OpenParenthesis) {
            return tokens;
        }
        self.t.next();
        let mut depth = 1;
        while let Some(token) = self.t.next() {
            match token {
                CssToken::OpenParenthesis => depth += 1,
                CssToken::CloseParenthesis => {
                    depth -= 1;
                    if depth == 0 {
                        return tokens;
                    }
                }
                _ => {}
            }
            tokens.push(token);
        }
        tokens
    }

    /// The contents of a style rule's block: its declarations, then any rules
    /// nested in it, with `&` standing for `selector`.
    /// https://www.w3.org/TR/css-nesting-1/
    fn parse_style_block(
        &mut self,
        selector: &Selector,
        media: &[usize],
        tokens: Vec<CssToken>,
    ) -> Vec<QualifiedRule> {
        let mut declaration_tokens: Vec<CssToken> = Vec::new();
        let mut nested: Vec<(Vec<CssToken>, Vec<CssToken>)> = Vec::new();
        let mut pending: Vec<CssToken> = Vec::new();
        let mut depth = 0;
        let mut parens = 0;
        let mut block: Vec<CssToken> = Vec::new();
        let mut prelude: Vec<CssToken> = Vec::new();
        for token in tokens {
            if depth > 0 {
                match token {
                    CssToken::OpenCurly => depth += 1,
                    CssToken::CloseCurly => {
                        depth -= 1;
                        if depth == 0 {
                            nested.push((
                                core::mem::take(&mut prelude),
                                core::mem::take(&mut block),
                            ));
                            continue;
                        }
                    }
                    _ => {}
                }
                block.push(token);
                continue;
            }
            match token {
                CssToken::OpenParenthesis => {
                    parens += 1;
                    pending.push(token);
                }
                CssToken::CloseParenthesis => {
                    parens -= 1;
                    pending.push(token);
                }
                CssToken::OpenCurly if parens == 0 => {
                    depth = 1;
                    prelude = core::mem::take(&mut pending);
                }
                CssToken::SemiColon if parens == 0 => {
                    declaration_tokens.append(&mut pending);
                    declaration_tokens.push(CssToken::SemiColon);
                }
                other => pending.push(other),
            }
        }
        declaration_tokens.append(&mut pending);

        let mut rule = QualifiedRule::new();
        rule.set_selector(selector.clone());
        rule.media = media.to_vec();
        rule.set_declarations(Self::from_tokens(declaration_tokens).consume_list_of_declarations());
        let mut rules = vec![rule];
        for (prelude, block) in nested {
            rules.extend(self.parse_nested(selector, media, prelude, block));
        }
        rules
    }

    fn parse_nested(
        &mut self,
        selector: &Selector,
        media: &[usize],
        prelude: Vec<CssToken>,
        block: Vec<CssToken>,
    ) -> Vec<QualifiedRule> {
        let first = prelude.iter().find(|t| **t != CssToken::Whitespace);
        if let Some(CssToken::AtKeyword(keyword)) = first {
            let conditions: Vec<CssToken> = prelude
                .iter()
                .skip_while(|t| !matches!(t, CssToken::AtKeyword(_)))
                .skip(1)
                .cloned()
                .collect();
            let mut media = media.to_vec();
            match keyword.as_str() {
                "media" => {
                    media.push(self.media_conditions.len());
                    self.media_conditions.push(parse_media_query_list(&conditions));
                }
                "container" => {
                    media.push(self.media_conditions.len());
                    self.media_conditions
                        .push(parse_media_query_list(&container_condition(&conditions)));
                }
                "supports" => {
                    if !supports_condition(&conditions) {
                        return Vec::new();
                    }
                }
                "layer" => {}
                _ => return Vec::new(),
            }
            return self.parse_style_block(selector, &media, block);
        }

        // Each alternative is relative to `&` unless it says where `&` goes.
        let mut tokens = Vec::new();
        for (index, part) in split_on_commas(&prelude).into_iter().enumerate() {
            if index > 0 {
                tokens.push(CssToken::Delim(','));
            }
            let mut part: Vec<CssToken> = part
                .iter()
                .skip_while(|t| **t == CssToken::Whitespace)
                .cloned()
                .collect();
            if !part.contains(&CssToken::Delim('&')) {
                tokens.push(CssToken::Delim('&'));
                tokens.push(CssToken::Whitespace);
            }
            tokens.append(&mut part);
        }
        let mut parser = Self::from_tokens(tokens);
        parser.nesting = Some(selector.clone());
        let nested_selector = parser.consume_selector_list();
        self.parse_style_block(&nested_selector, media, block)
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
        let mut pseudo_el: Option<PseudoElement> = None;
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
                Some(CssToken::Delim('&')) => {
                    self.t.next();
                    match self.nesting.clone() {
                        Some(parent) => parts.push(Selector::Is(Box::new(parent))),
                        None => never = true,
                    }
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
                    // `:not(...)` takes a real selector argument: parse it
                    // inline with the selector machinery, then expect `)`.
                    if name == "not" && self.t.peek() == Some(&CssToken::OpenParenthesis) {
                        let mut inner = Self::from_tokens(self.take_parenthesized());
                        inner.nesting = self.nesting.clone();
                        parts.push(Selector::Not(Box::new(inner.consume_selector_list())));
                        continue;
                    }
                    // `:is(list)` / `:where(list)`: matches when ANY of the
                    // alternatives matches. `:where` adds no specificity.
                    if matches!(
                        name.as_str(),
                        "is" | "where" | "matches" | "any" | "-webkit-any" | "-moz-any"
                    ) && self.t.peek() == Some(&CssToken::OpenParenthesis)
                    {
                        let mut inner = Self::from_tokens(self.take_parenthesized());
                        inner.nesting = self.nesting.clone();
                        let inner = Box::new(inner.consume_selector_list());
                        parts.push(if name == "where" {
                            Selector::Where(inner)
                        } else {
                            Selector::Is(inner)
                        });
                        continue;
                    }
                    // `:has(relative list)`: the arguments are relative to the
                    // subject, which `&` stands for here too.
                    // https://www.w3.org/TR/selectors-4/#relational
                    if name == "has" && self.t.peek() == Some(&CssToken::OpenParenthesis) {
                        let mut tokens = Vec::new();
                        for (index, part) in split_on_commas(&self.take_parenthesized())
                            .into_iter()
                            .enumerate()
                        {
                            if index > 0 {
                                tokens.push(CssToken::Delim(','));
                            }
                            tokens.push(CssToken::Delim('&'));
                            tokens.push(CssToken::Whitespace);
                            tokens.extend(
                                part.iter().skip_while(|t| **t == CssToken::Whitespace).cloned(),
                            );
                        }
                        let mut inner = Self::from_tokens(tokens);
                        inner.nesting = Some(Selector::HasSubject);
                        parts.push(Selector::Has(Rc::new(inner.consume_selector_list())));
                        continue;
                    }
                    // Other functional pseudos (`:nth-child(2n+1)`): capture
                    // the argument tokens (balanced parens).
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
                    // `:hover` is live (the renderer tracks the pointer and
                    // re-styles); the other interaction pseudo-classes have no
                    // state behind them yet and never match. Pseudo-elements
                    // (generated content like ::before/::after) have no box in
                    // this engine — applying their declarations to the element
                    // itself would leak decoration styles. Legacy single-colon
                    // spellings of before/after count as pseudo-elements too.
                    // `:focus-visible` stays inert: it means "focused *and*
                    // the UA would show a focus ring", which for a mouse click
                    // is false — treating it as `:focus` would paint rings real
                    // browsers do not. `:active` has no state behind it yet.
                    let interactive = matches!(name.as_str(), "active" | "focus-visible");
                    // ::before / ::after generate content boxes; capture the
                    // kind (the rule is realized by the layout-tree builder).
                    // Legacy single-colon `:before`/`:after` count too.
                    if (pseudo_element || name == "before" || name == "after")
                        && matches!(name.as_str(), "before" | "after")
                    {
                        pseudo_el = Some(if name == "before" {
                            PseudoElement::Before
                        } else {
                            PseudoElement::After
                        });
                        continue;
                    }
                    let legacy_pseudo_element =
                        matches!(name.as_str(), "selection" | "placeholder" | "first-line"
                            | "first-letter" | "marker" | "backdrop");
                    if interactive || pseudo_element || legacy_pseudo_element {
                        never = true;
                    } else {
                        match name.as_str() {
                            "hover" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::Hover))
                            }
                            "focus" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::Focus))
                            }
                            "focus-within" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::FocusWithin))
                            }
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
                            "first-of-type" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::FirstOfType))
                            }
                            "last-of-type" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::LastOfType))
                            }
                            "only-of-type" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::OnlyOfType))
                            }
                            "nth-child" | "nth-last-child" | "nth-of-type"
                            | "nth-last-of-type" => match parse_nth_formula(&args) {
                                Some((a, b)) => {
                                    let kind = match name.as_str() {
                                        "nth-child" => PseudoClassKind::NthChild(a, b),
                                        "nth-last-child" => PseudoClassKind::NthLastChild(a, b),
                                        "nth-of-type" => PseudoClassKind::NthOfType(a, b),
                                        _ => PseudoClassKind::NthLastOfType(a, b),
                                    };
                                    parts.push(Selector::PseudoClass(kind));
                                }
                                // Unparsable formula: never match rather than
                                // over-match.
                                None => never = true,
                            },
                            "link" | "any-link" | "-webkit-any-link" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::Link))
                            }
                            "checked" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::Checked))
                            }
                            "disabled" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::Disabled))
                            }
                            "enabled" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::Enabled))
                            }
                            "empty" => parts.push(Selector::PseudoClass(PseudoClassKind::Empty)),
                            "required" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::Required))
                            }
                            "optional" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::Optional))
                            }
                            "read-only" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::ReadOnly))
                            }
                            "read-write" => {
                                parts.push(Selector::PseudoClass(PseudoClassKind::ReadWrite))
                            }
                            "placeholder-shown" => parts
                                .push(Selector::PseudoClass(PseudoClassKind::PlaceholderShown)),
                            "scope" => parts.push(Selector::PseudoClass(PseudoClassKind::Root)),
                            // True of every element this engine produces, and
                            // of every language a page tends to ask about.
                            "defined" | "lang" => {}
                            // `:visited` must not match, for privacy and so
                            // links keep their unvisited color; `:target` and
                            // the rest have no state behind them yet. An
                            // unknown pseudo-class makes the selector invalid.
                            _ => never = true,
                        }
                    }
                }
                _ => break,
            }
        }
        if never {
            return Selector::Never;
        }
        let host = match parts.len() {
            0 if saw_universal => Selector::Universal,
            // A bare `::before` (no host compound) targets any element.
            0 if pseudo_el.is_some() => Selector::Universal,
            0 => Selector::UnknownSelector,
            1 => parts.pop().expect("len checked"),
            _ => Selector::Compound(parts),
        };
        match pseudo_el {
            Some(pe) => Selector::PseudoElement(Box::new(host), pe),
            None => host,
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
                                let v = std::format!("{}", n);
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
                | Some(CssToken::Delim('&'))
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

    fn consume_qualified_rule(&mut self) -> Option<Vec<QualifiedRule>> {
        self.skip_whitespace();
        self.t.peek()?;
        let mut selector = self.consume_selector_list();

        // Error recovery: skip anything unparseable (attribute selectors,
        // stray delimiters) up to the rule block, poisoning the selector so
        // the partially-understood rule doesn't over-match.
        loop {
            match self.t.peek() {
                None => return None,
                Some(CssToken::OpenCurly) => {
                    self.t.next();
                    let block = self.take_block();
                    return Some(self.parse_style_block(&selector, &[], block));
                }
                Some(_) => {
                    selector = Selector::UnknownSelector;
                    self.t.next();
                }
            }
        }
    }

    fn consume_list_of_rules(&mut self) -> Vec<QualifiedRule> {
        self.consume_rules(false)
    }

    /// Consume rules until EOF, or — inside an `@media { ... }` block —
    /// until the matching `}`.
    fn consume_rules(&mut self, in_block: bool) -> Vec<QualifiedRule> {
        let mut rules = Vec::new();

        loop {
            let token = match self.t.peek() {
                Some(t) => t.clone(),
                None => return rules,
            };
            if in_block && token == CssToken::CloseCurly {
                self.t.next();
                return rules;
            }
            match token {
                // Whitespace between rules must be skipped HERE: if it falls
                // through to consume_qualified_rule, an at-rule behind it is
                // never seen by the AtKeyword arm and its block leaks.
                CssToken::Whitespace => {
                    self.t.next();
                }
                CssToken::AtKeyword(ref keyword)
                    if matches!(keyword.as_str(), "media" | "container" | "supports" | "layer") =>
                {
                    // Conditional groups keep their rules, tagged with whatever
                    // has to hold for them to apply. Rules become active per
                    // layout via `StyleSheet::filter_for_media`.
                    let keyword = keyword.clone();
                    self.t.next();
                    let mut prelude = Vec::new();
                    let mut has_block = false;
                    loop {
                        match self.t.next() {
                            None => return rules,
                            Some(CssToken::OpenCurly) => {
                                has_block = true;
                                break;
                            }
                            Some(CssToken::SemiColon) => break,
                            Some(t) => prelude.push(t),
                        }
                    }
                    // `@layer a, b;` only declares an order.
                    if !has_block {
                        continue;
                    }
                    let condition = match keyword.as_str() {
                        "media" => Some(parse_media_query_list(&prelude)),
                        "container" => {
                            Some(parse_media_query_list(&container_condition(&prelude)))
                        }
                        _ => None,
                    };
                    let index = condition.map(|condition| {
                        self.media_conditions.push(condition);
                        self.media_conditions.len() - 1
                    });
                    let inner = self.consume_rules(true);
                    if keyword == "supports" && !supports_condition(&prelude) {
                        continue;
                    }
                    for mut rule in inner {
                        rule.media.extend(index);
                        if keyword == "layer" {
                            rule.layered = true;
                        }
                        rules.push(rule);
                    }
                }
                CssToken::AtKeyword(ref keyword)
                    if keyword == "keyframes" || keyword.ends_with("-keyframes") =>
                {
                    // `@keyframes name { 0% {..} to {..} }` — kept aside from
                    // the rule list (its blocks are keyed by offset, not by
                    // selector) for `animation-name` to look up.
                    // Vendor-prefixed spellings (`@-webkit-keyframes`) are
                    // common and parse identically.
                    self.t.next();
                    if let Some(rule) = self.consume_keyframes_rule() {
                        self.keyframes.push(rule);
                    }
                }
                CssToken::AtKeyword(_keyword) => {
                    // Skip every other at-rule wholesale. The engine can't
                    // evaluate `@supports` / `@font-face` / `@import`
                    // preludes yet, and parsing their contents as top-level
                    // rules would leak spurious rules (the historical @media
                    // bug that collapsed HN's desktop layout).
                    self.consume_at_rule();
                }
                _ => {
                    match self.consume_qualified_rule() {
                        Some(mut parsed) => rules.append(&mut parsed),
                        None => return rules,
                    }
                }
            }
        }
    }

    /// Consume `@keyframes <name> { <selector> { <declarations> } ... }`.
    /// Keyframe selectors are percentages (or `from`/`to`), so each block is
    /// stored against its offset in 0..=1 rather than a CSS selector.
    /// Spec: CSS Animations L1 §4. https://www.w3.org/TR/css-animations-1/#keyframes
    fn consume_keyframes_rule(&mut self) -> Option<KeyframesRule> {
        self.skip_whitespace();
        let name = match self.t.next() {
            Some(CssToken::Ident(name)) => name,
            Some(CssToken::StringToken(name)) => name,
            _ => return None,
        };
        self.skip_whitespace();
        if self.t.next() != Some(CssToken::OpenCurly) {
            return None;
        }
        let mut frames: Vec<Keyframe> = Vec::new();
        loop {
            self.skip_whitespace();
            match self.t.peek() {
                None => break,
                Some(CssToken::CloseCurly) => {
                    self.t.next();
                    break;
                }
                _ => {}
            }
            // Selector: a comma-separated list of percentages / from / to.
            let mut offsets = Vec::new();
            loop {
                match self.t.next() {
                    None => return finish_keyframes(name, frames),
                    Some(CssToken::OpenCurly) => break,
                    Some(CssToken::Ident(word)) => {
                        match word.to_ascii_lowercase().as_str() {
                            "from" => offsets.push(0.0),
                            "to" => offsets.push(1.0),
                            _ => {}
                        }
                    }
                    // Percentages tokenize as a Dimension with unit "%".
                    Some(CssToken::Dimension(value, unit)) if unit == "%" => {
                        offsets.push(value / 100.0)
                    }
                    // Bare numbers appear in sloppy sheets (`50 { }`).
                    Some(CssToken::Number(value)) => offsets.push(value / 100.0),
                    _ => {}
                }
            }
            let declarations = self.consume_list_of_declarations();
            for offset in offsets {
                frames.push(Keyframe {
                    offset: offset.clamp(0.0, 1.0),
                    declarations: declarations.clone(),
                });
            }
        }
        finish_keyframes(name, frames)
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
                        r.media = rule.media.clone();
                        r.layered = rule.layered;
                        rules.push(r);
                    }
                }
                _ => rules.push(rule),
            }
        }
        sheet.set_rules(rules);
        sheet.media_conditions = core::mem::take(&mut self.media_conditions);
        sheet.keyframes = core::mem::take(&mut self.keyframes);
        sheet
    }
}

/// Split a token run at its top-level commas.
fn split_on_commas(tokens: &[CssToken]) -> Vec<&[CssToken]> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            CssToken::OpenParenthesis => depth += 1,
            CssToken::CloseParenthesis => depth -= 1,
            CssToken::Delim(',') if depth == 0 => {
                parts.push(&tokens[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&tokens[start..]);
    parts
}

/// An `@container` prelude as a media condition. Containers have no sizes while
/// the stylesheet is being filtered, so this measures against the viewport,
/// which is right for the common case of a container spanning the page. The
/// optional container name is dropped.
fn container_condition(prelude: &[CssToken]) -> Vec<CssToken> {
    prelude
        .iter()
        .skip_while(|t| matches!(t, CssToken::Whitespace | CssToken::Ident(_)))
        .cloned()
        .collect()
}

/// Whether this engine does something with `property`, for `@supports`.
fn is_known_property(property: &str) -> bool {
    matches!(
        property,
        "align-content" | "align-items" | "align-self" | "animation" | "animation-name"
            | "aspect-ratio" | "background" | "background-color" | "background-image"
            | "background-position" | "background-repeat" | "background-size" | "border"
            | "border-bottom" | "border-color" | "border-left" | "border-radius" | "border-right"
            | "border-style" | "border-top" | "border-width" | "bottom" | "box-shadow"
            | "box-sizing" | "clear" | "color" | "column-gap" | "display" | "flex" | "flex-basis"
            | "flex-direction" | "flex-flow" | "flex-grow" | "flex-shrink" | "flex-wrap" | "float"
            | "font" | "font-family" | "font-size" | "font-style" | "font-weight" | "gap" | "grid"
            | "grid-area" | "grid-auto-columns" | "grid-auto-flow" | "grid-auto-rows"
            | "grid-column" | "grid-row" | "grid-template" | "grid-template-areas"
            | "grid-template-columns" | "grid-template-rows" | "height" | "inset"
            | "justify-content" | "justify-items" | "justify-self" | "left" | "letter-spacing"
            | "line-height" | "list-style" | "list-style-type" | "margin" | "margin-bottom"
            | "margin-left" | "margin-right" | "margin-top" | "max-height" | "max-width"
            | "min-height" | "min-width" | "opacity" | "order" | "outline" | "overflow"
            | "overflow-x" | "overflow-y" | "padding" | "padding-bottom" | "padding-left"
            | "padding-right" | "padding-top" | "place-content" | "place-items" | "place-self"
            | "position" | "right" | "row-gap" | "text-align" | "text-decoration" | "text-indent"
            | "text-overflow" | "text-shadow" | "text-transform" | "top" | "transform"
            | "transition" | "vertical-align" | "visibility" | "white-space" | "width"
            | "word-spacing" | "z-index"
    )
}

/// Evaluate an `@supports` condition. A declaration holds when the property is
/// known; `display` also looks at its value, the one case pages commonly test
/// that this engine would otherwise answer wrongly.
/// https://www.w3.org/TR/css-conditional-3/#at-supports
fn supports_condition(tokens: &[CssToken]) -> bool {
    let tokens: Vec<&CssToken> = tokens.iter().filter(|t| **t != CssToken::Whitespace).collect();
    let mut position = 0;
    supports_or(&tokens, &mut position)
}

fn supports_or(tokens: &[&CssToken], position: &mut usize) -> bool {
    let mut result = supports_and(tokens, position);
    while matches!(tokens.get(*position), Some(CssToken::Ident(i)) if i.eq_ignore_ascii_case("or")) {
        *position += 1;
        let next = supports_and(tokens, position);
        result = result || next;
    }
    result
}

fn supports_and(tokens: &[&CssToken], position: &mut usize) -> bool {
    let mut result = supports_term(tokens, position);
    while matches!(tokens.get(*position), Some(CssToken::Ident(i)) if i.eq_ignore_ascii_case("and")) {
        *position += 1;
        let next = supports_term(tokens, position);
        result = result && next;
    }
    result
}

fn supports_term(tokens: &[&CssToken], position: &mut usize) -> bool {
    match tokens.get(*position) {
        Some(CssToken::Ident(i)) if i.eq_ignore_ascii_case("not") => {
            *position += 1;
            !supports_term(tokens, position)
        }
        // `selector(...)` and other functions: the selector forms pages test
        // are the ones this engine has.
        Some(CssToken::Ident(_)) => {
            *position += 1;
            skip_parens(tokens, position);
            true
        }
        Some(CssToken::OpenParenthesis) => {
            let declaration = matches!(tokens.get(*position + 2), Some(CssToken::Colon));
            if declaration {
                let property = match tokens.get(*position + 1) {
                    Some(CssToken::Ident(name)) => name.to_ascii_lowercase(),
                    _ => String::new(),
                };
                let value = match tokens.get(*position + 3) {
                    Some(CssToken::Ident(value)) => value.to_ascii_lowercase(),
                    _ => String::new(),
                };
                skip_parens(tokens, position);
                is_known_property(&property)
                    && !(property == "display"
                        && matches!(value.as_str(), "ruby" | "subgrid" | "masonry"))
            } else {
                *position += 1;
                let result = supports_or(tokens, position);
                if matches!(tokens.get(*position), Some(CssToken::CloseParenthesis)) {
                    *position += 1;
                }
                result
            }
        }
        _ => {
            *position += 1;
            false
        }
    }
}

fn skip_parens(tokens: &[&CssToken], position: &mut usize) {
    if !matches!(tokens.get(*position), Some(CssToken::OpenParenthesis)) {
        return;
    }
    let mut depth = 0;
    while let Some(token) = tokens.get(*position) {
        *position += 1;
        match token {
            CssToken::OpenParenthesis => depth += 1,
            CssToken::CloseParenthesis => {
                depth -= 1;
                if depth == 0 {
                    return;
                }
            }
            _ => {}
        }
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
    substitute_vars_depth(value, vars, 0)
}

fn substitute_vars_depth(
    value: &[CssToken],
    vars: &BTreeMap<String, Vec<CssToken>>,
    depth: u32,
) -> Vec<CssToken> {
    // Cycle/pathology guard: --a:var(--b);--b:var(--a) must terminate.
    if depth > 8 {
        return value.to_vec();
    }
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
        // A custom property whose value is (or contains a substituted)
        // `initial` is the guaranteed-invalid value: the var() must use its
        // fallback instead. This is also how the csstools light-dark()
        // polyfill toggles: the "off" branch is set to `initial` so the
        // fallback (light) color wins. The fallback itself may nest var().
        let is_invalid = |v: &[CssToken]| {
            v.iter().any(|t| matches!(t, CssToken::Ident(s) if s == "initial"))
                || v.iter().all(|t| matches!(t, CssToken::Whitespace))
        };
        // An invalid substitution with no fallback must POISON the value
        // (emit the guaranteed-invalid sentinel) so an outer var() sees it —
        // this is how the csstools light-dark() polyfill selects its branch.
        let mut emit_fallback_or_poison =
            |out: &mut Vec<CssToken>, fallback: &[CssToken]| {
                let fb = substitute_vars_depth(fallback, vars, depth + 1);
                if fb.iter().all(|t| matches!(t, CssToken::Whitespace)) {
                    out.push(CssToken::Ident("initial".to_string()));
                } else {
                    out.extend(fb);
                }
            };
        match vars.get(&name) {
            Some(v) => {
                // Resolve nested var() FIRST; only then can we tell whether
                // the value collapses to guaranteed-invalid (`initial`).
                let resolved = substitute_vars_depth(v, vars, depth + 1);
                if is_invalid(&resolved) {
                    emit_fallback_or_poison(&mut out, &fallback);
                } else {
                    out.extend(resolved);
                }
            }
            None => emit_fallback_or_poison(&mut out, &fallback),
        }
        i = j + 1; // skip past the close paren
    }
    out
}

/// Which rules could match an element, found from its id, classes and tag instead
/// of trying every rule against it.
#[derive(Debug, Clone, Default)]
pub struct RuleIndex {
    by_id: HashMap<String, Vec<u32>>,
    by_class: HashMap<String, Vec<u32>>,
    by_tag: HashMap<String, Vec<u32>>,
    /// Rules that can match with no particular id, class or tag.
    unkeyed: Vec<u32>,
    /// `::before`/`::after` rules, which are found separately.
    pseudo_elements: Vec<u32>,
}

/// What an element has to have for a selector to match it.
enum RuleKey<'a> {
    Id(&'a str),
    Class(&'a str),
    Tag(&'a str),
}

/// The keys of which a matching element has at least one, or None if a selector
/// can match without any.
fn rule_keys(selector: &Selector) -> Option<Vec<RuleKey<'_>>> {
    match selector {
        Selector::IdSelector(id) => Some(vec![RuleKey::Id(id)]),
        Selector::ClassSelector(class) => Some(vec![RuleKey::Class(class)]),
        Selector::TypeSelector(tag) => Some(vec![RuleKey::Tag(tag)]),
        Selector::Never => Some(Vec::new()),
        // Any one part will do, since all have to match. The rarest kind is best.
        Selector::Compound(parts) => {
            let rank = |keys: &Vec<RuleKey<'_>>| match keys.first() {
                Some(RuleKey::Id(_)) => 0,
                Some(RuleKey::Class(_)) => 1,
                _ => 2,
            };
            parts
                .iter()
                .filter_map(rule_keys)
                .filter(|keys| !keys.is_empty())
                .min_by_key(rank)
        }
        Selector::Descendant(_, subject)
        | Selector::Child(_, subject)
        | Selector::NextSibling(_, subject)
        | Selector::SubsequentSibling(_, subject) => rule_keys(subject),
        Selector::Is(inner) | Selector::Where(inner) => rule_keys(inner),
        Selector::List(alternatives) => {
            let mut keys = Vec::new();
            for alternative in alternatives {
                keys.extend(rule_keys(alternative)?);
            }
            Some(keys)
        }
        _ => None,
    }
}

/// Built on first use: stylesheets aren't touched after parsing.
#[derive(Debug, Clone, Default)]
pub struct RuleIndexCell(std::cell::OnceCell<RuleIndex>);

impl PartialEq for RuleIndexCell {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct StyleSheet {
    pub rules: Vec<QualifiedRule>,
    /// `@media` conditions referenced by `QualifiedRule::media`.
    pub media_conditions: Vec<MediaQueryList>,
    /// `@keyframes` rules, looked up by `animation-name`.
    pub keyframes: Vec<KeyframesRule>,
    index: RuleIndexCell,
}

impl StyleSheet {
    pub fn new() -> Self {
        Self {
            rules: Vec::new(),
            media_conditions: Vec::new(),
            keyframes: Vec::new(),
            index: RuleIndexCell::default(),
        }
    }

    fn index(&self) -> &RuleIndex {
        self.index.0.get_or_init(|| {
            let mut index = RuleIndex::default();
            for (i, rule) in self.rules.iter().enumerate() {
                let i = i as u32;
                if matches!(rule.selector, Selector::PseudoElement(..)) {
                    index.pseudo_elements.push(i);
                    continue;
                }
                let Some(keys) = rule_keys(&rule.selector) else {
                    index.unkeyed.push(i);
                    continue;
                };
                for key in keys {
                    let (map, name) = match key {
                        RuleKey::Id(name) => (&mut index.by_id, name),
                        RuleKey::Class(name) => (&mut index.by_class, name),
                        RuleKey::Tag(name) => (&mut index.by_tag, name),
                    };
                    let list = map.entry(name.to_string()).or_default();
                    // An alternative of the same rule may have the same key.
                    if list.last() != Some(&i) {
                        list.push(i);
                    }
                }
            }
            index
        })
    }

    /// The rules that could match an element with this id, classes and tag, in
    /// document order. Every rule not returned certainly doesn't match it.
    pub fn candidate_rules<'a>(
        &self,
        id: Option<&str>,
        classes: impl Iterator<Item = &'a str>,
        tag: &str,
    ) -> Vec<&QualifiedRule> {
        let index = self.index();
        let mut found: Vec<u32> = index.unkeyed.clone();
        if let Some(rules) = id.and_then(|id| index.by_id.get(id)) {
            found.extend_from_slice(rules);
        }
        for class in classes {
            if let Some(rules) = index.by_class.get(class) {
                found.extend_from_slice(rules);
            }
        }
        if let Some(rules) = index.by_tag.get(tag) {
            found.extend_from_slice(rules);
        }
        found.sort_unstable();
        found.dedup();
        found.iter().map(|&i| &self.rules[i as usize]).collect()
    }

    /// The `::before` and `::after` rules, in document order.
    pub fn pseudo_element_rules(&self) -> impl Iterator<Item = &QualifiedRule> {
        self.index()
            .pseudo_elements
            .iter()
            .map(|&i| &self.rules[i as usize])
    }

    /// The `@keyframes` rule named `name`, if the document declares one. A
    /// later declaration of the same name wins (CSS Animations L1 §4).
    pub fn keyframes_named(&self, name: &str) -> Option<&KeyframesRule> {
        self.keyframes.iter().rev().find(|rule| rule.name == name)
    }

    /// Whether any rule is conditioned on `:hover`. The renderer uses this to
    /// skip pointer tracking entirely on documents that can't react to it —
    /// hover handling costs a hit-test plus a re-style, and most pages don't
    /// need one.
    pub fn uses_hover(&self) -> bool {
        self.rules.iter().any(|rule| rule.selector.uses_hover())
    }

    /// As `uses_hover`, for `:focus` / `:focus-within`.
    pub fn uses_focus(&self) -> bool {
        self.rules.iter().any(|rule| rule.selector.uses_focus())
    }

    pub fn set_rules(&mut self, rules: Vec<QualifiedRule>) {
        self.rules = rules;
    }

    /// This sheet with the browser's own rules in front of it.
    pub fn with_user_agent_rules(&self) -> StyleSheet {
        thread_local! {
            static USER_AGENT_RULES: Vec<QualifiedRule> = {
                let css = include_str!("../style/ua.css").to_string();
                let mut rules = CssParser::new(CssTokenizer::new(css)).parse_stylesheet().rules;
                for rule in &mut rules {
                    rule.user_agent = true;
                }
                rules
            };
        }
        let mut sheet = self.clone();
        USER_AGENT_RULES.with(|rules| {
            sheet.rules.splice(0..0, rules.iter().cloned());
        });
        sheet
    }

    /// The rules that apply in `ctx`: unconditional rules plus the rules of
    /// matching `@media` blocks, in document order, with conditions
    /// resolved away.
    pub fn filter_for_media(&self, ctx: &MediaContext) -> StyleSheet {
        let active: Vec<bool> = self
            .media_conditions
            .iter()
            .map(|q| q.matches(ctx))
            .collect();
        StyleSheet {
            rules: self
                .rules
                .iter()
                .filter(|r| {
                    r.media
                        .iter()
                        .all(|i| active.get(*i).copied().unwrap_or(false))
                })
                .map(|r| {
                    let mut r = r.clone();
                    r.media.clear();
                    r
                })
                .collect(),
            media_conditions: Vec::new(),
            // Keyframes are unconditional (a `@keyframes` inside `@media` is
            // skipped by the parser), so they survive the filter as-is.
            keyframes: self.keyframes.clone(),
            index: RuleIndexCell::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct QualifiedRule {
    pub selector: Selector,
    pub declarations: Vec<Declaration>,
    /// Indexes into `StyleSheet::media_conditions` of the `@media` blocks this
    /// rule lives inside. All of them have to match.
    pub media: Vec<usize>,
    /// Declared inside `@layer`: loses to unlayered rules of the same origin.
    pub layered: bool,
    /// From the browser's own stylesheet: loses to every page rule.
    pub user_agent: bool,
}

impl QualifiedRule {
    pub fn new() -> Self {
        Self {
            selector: Selector::TypeSelector("".to_string()),
            declarations: Vec::new(),
            media: Vec::new(),
            layered: false,
            user_agent: false,
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
    /// `:not(...)` — matches when the inner selector does NOT match.
    /// https://www.w3.org/TR/selectors-4/#negation
    Not(Box<Selector>),
    /// `:is(...)` (also `:where(...)`/legacy `:matches(...)`) — matches when
    /// ANY alternative in the list matches.
    /// https://www.w3.org/TR/selectors-4/#matches
    Is(Box<Selector>),
    /// `:where(...)`: `:is` with no specificity.
    Where(Box<Selector>),
    /// `:has(...)`: some element related to the subject matches. The argument is
    /// a selector over those elements, with [`Selector::HasSubject`] marking
    /// where the subject has to be.
    /// https://www.w3.org/TR/selectors-4/#relational
    Has(Rc<Selector>),
    /// The element `:has()` is being tested against.
    HasSubject,
    /// `host::before` / `host::after` — never matches a real element (so its
    /// declarations don't leak onto the host); the layout tree builder uses
    /// `pseudo_element_target` to find these and synthesize a generated box.
    PseudoElement(Box<Selector>, PseudoElement),
}

/// `::before` / `::after` generated-content pseudo-elements.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PseudoElement {
    Before,
    After,
}

/// Supported structural pseudo-classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PseudoClassKind {
    /// `:hover` — the element under the pointer, or an ancestor of it.
    /// Matched against the hover chain the renderer publishes before each
    /// style pass. https://www.w3.org/TR/selectors-4/#the-hover-pseudo
    Hover,
    /// `:focus` — the element with keyboard focus.
    /// https://www.w3.org/TR/selectors-4/#the-focus-pseudo
    Focus,
    /// `:focus-within` — the focused element or an ancestor of it.
    FocusWithin,
    /// `:root` — the document's root element (`<html>`).
    Root,
    /// `:link` — a hyperlink. Nothing here counts as visited, so also `:any-link`.
    Link,
    Checked,
    Disabled,
    Enabled,
    Empty,
    Required,
    Optional,
    ReadOnly,
    ReadWrite,
    PlaceholderShown,
    FirstChild,
    LastChild,
    OnlyChild,
    /// `:nth-child(An+B)` — matches the (An+B)-th element child for some
    /// integer n ≥ 0 (1-based). `odd` = 2n+1, `even` = 2n.
    NthChild(i64, i64),
    /// `:nth-last-child(An+B)` — same, counting from the end.
    NthLastChild(i64, i64),
    /// `*-of-type` family: like the child variants but counting only siblings
    /// with the SAME tag name.
    FirstOfType,
    LastOfType,
    OnlyOfType,
    NthOfType(i64, i64),
    NthLastOfType(i64, i64),
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

/// One `@keyframes name { ... }` rule: its blocks keyed by offset rather than
/// by selector. Spec: CSS Animations L1 §4.
/// https://www.w3.org/TR/css-animations-1/#keyframes
#[derive(Debug, Clone, PartialEq)]
pub struct KeyframesRule {
    /// The `animation-name` this rule is addressed by (case-sensitive, as in
    /// the spec's custom-ident matching).
    pub name: String,
    /// Frames sorted by offset, each offset appearing once (a later block for
    /// the same offset replaces the earlier one, per document order).
    pub frames: Vec<Keyframe>,
}

/// One block inside `@keyframes`, at `offset` in 0..=1.
#[derive(Debug, Clone, PartialEq)]
pub struct Keyframe {
    pub offset: f64,
    pub declarations: Vec<Declaration>,
}

/// Sort/dedupe the collected frames and reject a rule with nothing usable.
fn finish_keyframes(name: String, mut frames: Vec<Keyframe>) -> Option<KeyframesRule> {
    if frames.is_empty() {
        return None;
    }
    // A later block at the same offset wins, so keep the last of each.
    frames.sort_by(|a, b| {
        a.offset
            .partial_cmp(&b.offset)
            .unwrap_or(core::cmp::Ordering::Equal)
    });
    frames.dedup_by(|later, earlier| {
        if (later.offset - earlier.offset).abs() < f64::EPSILON {
            earlier.declarations = core::mem::take(&mut later.declarations);
            true
        } else {
            false
        }
    });
    Some(KeyframesRule { name, frames })
}

impl Selector {
    /// Whether this selector (or any part of it) tests `:focus`-family state.
    pub fn uses_focus(&self) -> bool {
        self.tests_pseudo(&|kind| {
            matches!(kind, PseudoClassKind::Focus | PseudoClassKind::FocusWithin)
        })
    }

    /// Whether this selector (or any part of it) tests `:hover`.
    pub fn uses_hover(&self) -> bool {
        self.tests_pseudo(&|kind| matches!(kind, PseudoClassKind::Hover))
    }

    /// Whether any pseudo-class in this selector satisfies `want`.
    fn tests_pseudo(&self, want: &dyn Fn(&PseudoClassKind) -> bool) -> bool {
        match self {
            Self::PseudoClass(kind) => want(kind),
            Self::Compound(parts) | Self::List(parts) => {
                parts.iter().any(|part| part.tests_pseudo(want))
            }
            Self::Descendant(a, b)
            | Self::Child(a, b)
            | Self::NextSibling(a, b)
            | Self::SubsequentSibling(a, b) => a.tests_pseudo(want) || b.tests_pseudo(want),
            Self::Not(inner)
            | Self::Is(inner)
            | Self::Where(inner)
            | Self::PseudoElement(inner, _) => inner.tests_pseudo(want),
            Self::Has(inner) => inner.tests_pseudo(want),
            _ => false,
        }
    }


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
            Selector::Universal
            | Selector::UnknownSelector
            | Selector::Never
            | Selector::Where(_)
            | Selector::HasSubject => (0, 0, 0),
            Selector::Has(inner) => inner.specificity_abc(),
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
            // :not() takes the specificity of its argument (Selectors L4 §17).
            Selector::Not(inner) => inner.specificity_abc(),
            Selector::Is(inner) => inner.specificity_abc(),
            // A pseudo-element counts as a type selector, added to the host.
            Selector::PseudoElement(host, _) => {
                let (a, b, c) = host.specificity_abc();
                (a, b, c + 1)
            }
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
    use std::vec;

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
    fn test_media_block_rules_are_kept_and_filtered_by_viewport() {
        let style = "td { color: blue; } @media (max-width: 600px) { td { width: 10px; display: block; } } p { color: red; }".to_string();
        let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
        // All three rules survive in document order; the middle one carries
        // its @media condition index (it must never leak as unconditional —
        // that was the historical bug that collapsed HN's desktop layout).
        let tagged: std::vec::Vec<_> = cssom
            .rules
            .iter()
            .map(|r| (r.selector.clone(), r.media.clone()))
            .collect();
        assert_eq!(
            tagged,
            std::vec![
                (Selector::TypeSelector("td".to_string()), std::vec![]),
                (Selector::TypeSelector("td".to_string()), std::vec![0]),
                (Selector::TypeSelector("p".to_string()), std::vec![]),
            ],
            "got rules: {:?}",
            cssom.rules
        );
        assert_eq!(cssom.media_conditions.len(), 1);

        let desktop = MediaContext {
            viewport_width: 1024.0,
            viewport_height: 768.0,
            prefers_dark: false,
        };
        let phone = MediaContext {
            viewport_width: 390.0,
            viewport_height: 800.0,
            prefers_dark: false,
        };
        assert_eq!(cssom.filter_for_media(&desktop).rules.len(), 2);
        let phone_rules = cssom.filter_for_media(&phone).rules;
        assert_eq!(phone_rules.len(), 3);
        assert!(phone_rules.iter().all(|r| r.media.is_empty()));
    }

    #[test]
    fn keyframes_are_parsed_and_looked_up_by_name() {
        let style = "p{color:red} @keyframes fade { from { opacity: 1; } 50% { opacity: 0.5; } \
                     to { opacity: 0; } } h1{color:blue}"
            .to_string();
        let sheet = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();

        // The at-rule must not leak its blocks into the rule list.
        assert_eq!(sheet.rules.len(), 2, "only p and h1 are qualified rules");

        let frames = &sheet.keyframes_named("fade").expect("@keyframes fade").frames;
        let offsets: Vec<f64> = frames.iter().map(|f| f.offset).collect();
        assert_eq!(offsets, vec![0.0, 0.5, 1.0], "from/50%/to sorted by offset");
        assert_eq!(frames[1].declarations[0].property, "opacity");
        assert!(sheet.keyframes_named("missing").is_none());
    }

    #[test]
    fn keyframes_selector_lists_and_vendor_prefixes() {
        let style = "@-webkit-keyframes pulse { 0%, 100% { opacity: 1; } 50% { opacity: 0; } }"
            .to_string();
        let sheet = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
        let frames = &sheet.keyframes_named("pulse").expect("prefixed rule").frames;
        // `0%, 100%` declares both ends in one block.
        assert_eq!(
            frames.iter().map(|f| f.offset).collect::<Vec<_>>(),
            vec![0.0, 0.5, 1.0]
        );
    }

    #[test]
    fn test_other_at_rules_still_skipped() {
        let style = "@import url(x.css); @font-face { font-family: X; src: url(x.woff); } td { color: blue; } @supports (display: grid) { p { color: red; } }".to_string();
        let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
        let selectors: std::vec::Vec<_> = cssom.rules.iter().map(|r| r.selector.clone()).collect();
        assert_eq!(
            selectors,
            std::vec![
                Selector::TypeSelector("td".to_string()),
                Selector::TypeSelector("p".to_string()),
            ],
            "@supports keeps its rules when the condition holds; the others are skipped"
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
