//! Data-driven HTML parser tests against the vendored html5lib-tests
//! tree-construction corpus (saba/testdata/html5lib-tests/, last upstream
//! revision before the corpus moved to WPT).
//!
//! The engine's parser implements a subset of the spec (no comments, no
//! doctype nodes, no namespaces, no fragment parsing, no adoption agency),
//! so most cases fail today. This runner asserts the pass count never
//! DECREASES: raise `BASELINE_PASSES` whenever parser work lands, never
//! lower it. Per-file counts print with `cargo test -p cosmo_engine
//! --test html5lib -- --nocapture`.

use cosmo_engine::renderer::dom::node::{Node, NodeKind};
use cosmo_engine::renderer::html::parser::HtmlParser;
use cosmo_engine::renderer::html::token::HtmlTokenizer;
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

/// Minimum number of corpus cases that must pass. Measured on 2026-07-13;
/// raise together with parser improvements (plan 2.6 targets >= 85% of
/// non-script, non-fragment cases).
const BASELINE_PASSES: usize = 79;

struct Case {
    file: String,
    data: String,
    expected: String,
    script_dependent: bool,
    fragment: bool,
}

fn parse_dat(file: &str, content: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    let mut section = "";
    let mut data = String::new();
    let mut document = String::new();
    let mut fragment = false;
    let mut script_dependent = false;
    let mut in_case = false;

    let mut flush = |data: &mut String,
                     document: &mut String,
                     fragment: &mut bool,
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
                fragment: *fragment,
            });
        }
        data.clear();
        document.clear();
        *fragment = false;
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
            "#script-off" => section = "script",
            "#document-fragment" => {
                fragment = true;
                section = "fragment";
            }
            _ => match section {
                "data" => {
                    data.push_str(line);
                    data.push('\n');
                }
                "document" => {
                    document.push_str(line);
                    document.push('\n');
                }
                _ => {}
            },
        }
    }
    flush(&mut data, &mut document, &mut fragment, &mut script_dependent, &mut in_case);
    cases
}

fn serialize(node: &Option<Rc<RefCell<Node>>>, depth: usize, out: &mut String) {
    let mut current = node.clone();
    while let Some(n) = current {
        match n.borrow().kind() {
            NodeKind::Document => {}
            NodeKind::Element(ref e) => {
                out.push_str("| ");
                out.push_str(&"  ".repeat(depth));
                out.push('<');
                out.push_str(e.tag_name());
                out.push_str(">\n");
                let mut attrs = e.attributes();
                attrs.sort_by(|a, b| a.name().cmp(&b.name()));
                for a in attrs {
                    out.push_str("| ");
                    out.push_str(&"  ".repeat(depth + 1));
                    out.push_str(&a.name());
                    out.push_str("=\"");
                    out.push_str(&a.value());
                    out.push_str("\"\n");
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
        let child_depth = match n.borrow().kind() {
            NodeKind::Document => depth,
            _ => depth + 1,
        };
        serialize(&n.borrow().first_child(), child_depth, out);
        let next = n.borrow().next_sibling();
        current = next;
    }
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
            if case.fragment || case.script_dependent {
                skipped += 1;
                continue;
            }
            total += 1;
            file_total += 1;
            let window = HtmlParser::new(HtmlTokenizer::new(case.data.clone())).construct_tree();
            let document = window.borrow().document();
            let mut actual = String::new();
            serialize(&document.borrow().first_child(), 0, &mut actual);
            let actual = actual.trim_end_matches('\n');
            if actual == case.expected {
                passed += 1;
                file_pass += 1;
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
