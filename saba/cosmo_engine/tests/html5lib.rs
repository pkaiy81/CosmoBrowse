//! Data-driven HTML parser tests against the vendored html5lib-tests
//! tree-construction corpus (saba/testdata/html5lib-tests/, last upstream
//! revision before the corpus moved to WPT).
//!
//! This runner asserts the pass count never DECREASES: raise `BASELINE_PASSES`
//! whenever parser work lands, never lower it. Per-file counts print with
//! `cargo test -p cosmo_engine --test html5lib -- --nocapture`.

use cosmo_engine::renderer::dom::node::{Namespace, Node, NodeKind};
use cosmo_engine::renderer::html::parser::HtmlParser;
use cosmo_engine::renderer::html::token::HtmlTokenizer;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// Minimum number of corpus cases that must pass. Measured on 2026-11-07;
/// raise together with parser improvements (plan 2.6 targets >= 85% of
/// non-script, non-fragment cases).
const BASELINE_PASSES: usize = 1752;

struct Case {
    file: String,
    data: String,
    expected: String,
    script_dependent: bool,
    /// The context element for a fragment case: its namespace prefix, if any, and tag.
    fragment: Option<String>,
}

fn parse_dat(file: &str, content: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut section = "";
    let mut data = String::new();
    let mut document = String::new();
    let mut fragment: Option<String> = None;
    let mut script_dependent = false;
    let mut in_case = false;

    let mut flush = |data: &mut String,
                     document: &mut String,
                     fragment: &mut Option<String>,
                     script: &mut bool,
                     in_case: &mut bool| {
        if *in_case {
            // #data carries one trailing newline from the line-based
            // accumulation; #document additionally accumulates the blank
            // separator line before the next case, so trim ALL trailing
            // newlines there (the actual tree is trimmed the same way).
            let d = data.strip_suffix('\n').unwrap_or(data).to_string();
            let doc = document.trim_end_matches('\n').to_string();
            cases.push(Case {
                file: file.to_string(),
                data: d,
                expected: doc,
                script_dependent: *script,
                fragment: fragment.take(),
            });
        }
        data.clear();
        document.clear();
        *fragment = None;
        *script = false;
        *in_case = false;
    };

    for line in content.lines() {
        match line {
            "#data" => {
                flush(&mut data, &mut document, &mut fragment, &mut script_dependent, &mut in_case);
                section = "data";
                in_case = true;
            }
            "#errors" | "#new-errors" => section = "errors",
            "#document" => section = "document",
            "#script-on" => {
                script_dependent = true;
                section = "script";
            }
            "#script-off" => {
                script_dependent = true;
                section = "script";
            }
            "#document-fragment" => section = "fragment",
            _ => match section {
                "data" => {
                    data.push_str(line);
                    data.push('\n');
                }
                "document" => {
                    document.push_str(line);
                    document.push('\n');
                }
                "fragment" => fragment = Some(line.trim().to_string()),
                _ => {}
            },
        }
    }
    flush(&mut data, &mut document, &mut fragment, &mut script_dependent, &mut in_case);
    cases
}

/// `xlink:href` on a foreign element serializes as `xlink href`, the format's way
/// of showing an attribute's namespace.
fn foreign_attribute_name(name: &str) -> String {
    match name {
        "xlink:actuate" | "xlink:arcrole" | "xlink:href" | "xlink:role" | "xlink:show"
        | "xlink:title" | "xlink:type" | "xml:lang" | "xml:space" | "xmlns:xlink" => {
            name.replacen(':', " ", 1)
        }
        other => other.to_string(),
    }
}

fn serialize(node: &Option<Rc<RefCell<Node>>>, depth: usize, out: &mut String) {
    let mut current = node.clone();
    while let Some(n) = current {
        let mut template_depth = None;
        match n.borrow().kind() {
            NodeKind::Document => {}
            NodeKind::Comment(ref text) => {
                out.push_str("| ");
                out.push_str(&"  ".repeat(depth));
                out.push_str("<!-- ");
                out.push_str(text);
                out.push_str(" -->\n");
            }
            NodeKind::Doctype { ref name, ref public_id, ref system_id } => {
                out.push_str("| ");
                out.push_str(&"  ".repeat(depth));
                out.push_str("<!DOCTYPE ");
                out.push_str(name);
                if !public_id.is_empty() || !system_id.is_empty() {
                    out.push_str(&format!(" \"{public_id}\" \"{system_id}\""));
                }
                out.push_str(">\n");
            }
            NodeKind::Element(ref e) => {
                out.push_str("| ");
                out.push_str(&"  ".repeat(depth));
                out.push('<');
                match e.namespace() {
                    Namespace::Html => {}
                    Namespace::Svg => out.push_str("svg "),
                    Namespace::MathMl => out.push_str("math "),
                }
                out.push_str(e.tag_name());
                out.push_str(">\n");
                let mut attrs: Vec<(String, String)> = e
                    .attributes()
                    .iter()
                    .map(|a| {
                        let name = if e.namespace() == Namespace::Html {
                            a.name()
                        } else {
                            foreign_attribute_name(&a.name())
                        };
                        (name, a.value())
                    })
                    .collect();
                attrs.sort();
                for (name, value) in attrs {
                    out.push_str("| ");
                    out.push_str(&"  ".repeat(depth + 1));
                    out.push_str(&name);
                    out.push_str("=\"");
                    out.push_str(&value);
                    out.push_str("\"\n");
                }
                // A template's children live in its content document.
                if e.tag_name() == "template" && e.namespace() == Namespace::Html {
                    out.push_str("| ");
                    out.push_str(&"  ".repeat(depth + 1));
                    out.push_str("content\n");
                    template_depth = Some(depth + 2);
                }
            }
            NodeKind::Text(ref t) => {
                out.push_str("| ");
                out.push_str(&"  ".repeat(depth));
                out.push('"');
                out.push_str(t);
                out.push_str("\"\n");
            }
        }
        let child_depth = template_depth.unwrap_or_else(|| match n.borrow().kind() {
            NodeKind::Document => depth,
            _ => depth + 1,
        });
        serialize(&n.borrow().first_child(), child_depth, out);
        let next = n.borrow().next_sibling();
        current = next;
    }
}

/// Serialize a node and its subtree, leaving its siblings out.
fn serialize_one(node: &Rc<RefCell<Node>>, depth: usize, out: &mut String) {
    let next = node.borrow().next_sibling();
    node.borrow_mut().set_next_sibling(None);
    serialize(&Some(node.clone()), depth, out);
    node.borrow_mut().set_next_sibling(next);
}

#[test]
fn tree_construction_corpus() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../testdata/html5lib-tests/tree-construction");
    let mut total = 0usize;
    let mut skipped = 0usize;
    let mut passed = 0usize;

    let mut entries: Vec<_> = std::fs::read_dir(&dir)
        .expect("vendored html5lib-tests missing")
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "dat"))
        .collect();
    entries.sort();

    for path in entries {
        let content = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => continue, // non-UTF8 corpus file (domjs-unsafe): skip
        };
        let file = path.file_name().unwrap().to_string_lossy().to_string();
        let mut file_pass = 0usize;
        let mut file_total = 0usize;
        for case in parse_dat(&file, &content) {
            if case.script_dependent {
                skipped += 1;
                continue;
            }
            total += 1;
            file_total += 1;
            let mut actual = String::new();
            match &case.fragment {
                Some(context) => {
                    let (namespace, tag) = match context.split_once(' ') {
                        Some(("svg", tag)) => (Namespace::Svg, tag),
                        Some(("math", tag)) => (Namespace::MathMl, tag),
                        _ => (Namespace::Html, context.as_str()),
                    };
                    let nodes =
                        HtmlParser::parse_fragment_in(tag, namespace, case.data.clone());
                    for node in nodes {
                        let mut single = String::new();
                        serialize_one(&node, 0, &mut single);
                        actual.push_str(&single);
                    }
                }
                None => {
                    let window =
                        HtmlParser::new(HtmlTokenizer::new(case.data.clone())).construct_tree();
                    let document = window.borrow().document();
                    serialize(&document.borrow().first_child(), 0, &mut actual);
                }
            }
            let actual = actual.trim_end_matches('\n');
            if actual == case.expected {
                passed += 1;
                file_pass += 1;
            } else if std::env::var("HTML5LIB_SHOW").as_deref() == Ok(file.as_str()) {
                println!("--- {}\n--- expected\n{}\n--- actual\n{}", case.data, case.expected, actual);
            }
        }
        println!("{file}: {file_pass}/{file_total}");
    }

    println!("html5lib tree-construction: {passed}/{total} passed ({skipped} skipped)");
    assert!(total > 1000, "corpus did not load (only {total} cases)");
    assert!(
        passed >= BASELINE_PASSES,
        "pass count regressed: {passed} < baseline {BASELINE_PASSES}"
    );
}
