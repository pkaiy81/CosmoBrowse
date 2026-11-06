//! What a click on a form control does, and form submission.
//! https://html.spec.whatwg.org/multipage/form-control-infrastructure.html

use cosmo_engine::renderer::dom::forms::{
    attribute, descendants, document_root, form_data, input_type, is_disabled, tag,
};
use cosmo_engine::renderer::dom::node::{Node, NodeKind};
use std::cell::RefCell;
use std::rc::Rc;
use url::Url;

type NodeRef = Rc<RefCell<Node>>;

pub(crate) fn set_attribute(node: &NodeRef, name: &str, value: &str) {
    if let NodeKind::Element(ref mut e) = node.borrow_mut().kind {
        e.set_attribute(name, value);
    }
}

pub(crate) fn remove_attribute(node: &NodeRef, name: &str) {
    if let NodeKind::Element(ref mut e) = node.borrow_mut().kind {
        e.remove_attribute(name);
    }
}

fn ancestors_and_self(node: &NodeRef) -> Vec<NodeRef> {
    cosmo_engine::renderer::dom::forms::ancestors_and_self(node)
}

/// The control a click on a `<label>` activates.
fn labeled_control(label: &NodeRef) -> Option<NodeRef> {
    let is_labelable = |n: &NodeRef| {
        matches!(tag(n).as_str(), "input" | "select" | "textarea" | "button")
            && !(tag(n) == "input" && input_type(n) == "hidden")
    };
    if let Some(id) = attribute(label, "for") {
        let root = document_root(label);
        let mut all = Vec::new();
        descendants(&root, &mut all);
        return all
            .into_iter()
            .find(|n| attribute(n, "id").as_deref() == Some(id.as_str()))
            .filter(is_labelable);
    }
    let mut inside = Vec::new();
    descendants(label, &mut inside);
    inside.into_iter().find(is_labelable)
}

/// The form a control belongs to.
fn form_owner(node: &NodeRef) -> Option<NodeRef> {
    if let Some(id) = attribute(node, "form") {
        let root = document_root(node);
        let mut all = Vec::new();
        descendants(&root, &mut all);
        return all
            .into_iter()
            .find(|n| tag(n) == "form" && attribute(n, "id").as_deref() == Some(id.as_str()));
    }
    ancestors_and_self(node).into_iter().skip(1).find(|n| tag(n) == "form")
}

/// What activating the clicked element should do.
pub(crate) enum Activation {
    None,
    /// A checkbox or radio button changed.
    Toggled(NodeRef),
    /// Open or close a `<select>`'s list.
    ToggleSelect(NodeRef),
    /// Open or close a `<details>`.
    ToggleDetails(NodeRef),
    Submit { form: NodeRef, submitter: Option<NodeRef> },
    /// Move the focus into a text control, for a click on its label.
    FocusField(NodeRef),
}

/// Find what a click on `target` activates, walking up to the nearest element
/// that has an activation behaviour, and carry out what doesn't involve the page.
/// https://html.spec.whatwg.org/multipage/interaction.html#activation-behavior
pub(crate) fn activate(target: &NodeRef) -> Activation {
    for node in ancestors_and_self(target) {
        match tag(&node).as_str() {
            "label" => {
                return match labeled_control(&node) {
                    Some(control) => activate_control(&control),
                    None => Activation::None,
                };
            }
            "input" | "button" | "select" => return activate_control(&node),
            "summary" => {
                let parent = node.borrow().parent().upgrade();
                if let Some(details) = parent.filter(|p| tag(p) == "details") {
                    return Activation::ToggleDetails(details);
                }
            }
            // A link inside something else takes the click for itself.
            "a" if attribute(&node, "href").is_some() => return Activation::None,
            _ => {}
        }
    }
    Activation::None
}

fn activate_control(control: &NodeRef) -> Activation {
    if is_disabled(control) {
        return Activation::None;
    }
    match tag(control).as_str() {
        "select" => Activation::ToggleSelect(control.clone()),
        "textarea" => Activation::FocusField(control.clone()),
        "button" => {
            let kind = attribute(control, "type").unwrap_or_default().to_ascii_lowercase();
            if kind == "button" || kind == "reset" {
                return Activation::None;
            }
            match form_owner(control) {
                Some(form) => Activation::Submit {
                    form,
                    submitter: Some(control.clone()),
                },
                None => Activation::None,
            }
        }
        "input" => match input_type(control).as_str() {
            "checkbox" => {
                if attribute(control, "checked").is_some() {
                    remove_attribute(control, "checked");
                } else {
                    set_attribute(control, "checked", "");
                }
                Activation::Toggled(control.clone())
            }
            "radio" => {
                if attribute(control, "checked").is_some() {
                    return Activation::None;
                }
                let name = attribute(control, "name");
                let scope = form_owner(control).unwrap_or_else(|| document_root(control));
                let mut all = Vec::new();
                descendants(&scope, &mut all);
                for other in all {
                    if tag(&other) == "input"
                        && input_type(&other) == "radio"
                        && attribute(&other, "name") == name
                    {
                        remove_attribute(&other, "checked");
                    }
                }
                set_attribute(control, "checked", "");
                Activation::Toggled(control.clone())
            }
            "submit" | "image" => match form_owner(control) {
                Some(form) => Activation::Submit {
                    form,
                    submitter: Some(control.clone()),
                },
                None => Activation::None,
            },
            "button" | "reset" | "file" | "hidden" | "range" | "color" => Activation::None,
            _ => Activation::FocusField(control.clone()),
        },
        _ => Activation::None,
    }
}

/// Pressing Enter in a text field submits its form.
/// https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#implicit-submission
pub(crate) fn implicit_submission(field: &NodeRef) -> Option<Activation> {
    let form = form_owner(field)?;
    let mut all = Vec::new();
    descendants(&form, &mut all);
    let submitter = all.into_iter().find(|n| match tag(n).as_str() {
        "button" => {
            let kind = attribute(n, "type").unwrap_or_default().to_ascii_lowercase();
            kind != "button" && kind != "reset"
        }
        "input" => matches!(input_type(n).as_str(), "submit" | "image"),
        _ => false,
    });
    if submitter.as_ref().is_some_and(is_disabled) {
        return None;
    }
    Some(Activation::Submit {
        form,
        submitter,
    })
}

/// Where a submission goes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Submission {
    pub url: String,
    /// The urlencoded body of a POST. A GET's data is in the URL.
    pub post_body: Option<String>,
    pub target: Option<String>,
}

/// The request a submission makes. The submitter can override the form's
/// `action`, `method` and `target`.
/// https://html.spec.whatwg.org/multipage/form-control-infrastructure.html#form-submission-algorithm
pub(crate) fn build_submission(
    form: &NodeRef,
    submitter: Option<&NodeRef>,
    document_url: &str,
) -> Option<Submission> {
    let overriding = |form_attr: &str, name: &str| {
        submitter
            .and_then(|s| attribute(s, form_attr))
            .or_else(|| attribute(form, name))
    };
    let base = Url::parse(document_url).ok()?;
    let action = overriding("formaction", "action").unwrap_or_default();
    let mut url = if action.trim().is_empty() {
        base.clone()
    } else {
        base.join(action.trim()).ok()?
    };
    url.set_fragment(None);
    let method = overriding("formmethod", "method")
        .unwrap_or_default()
        .to_ascii_lowercase();
    let body: String = url::form_urlencoded::Serializer::new(String::new())
        .extend_pairs(form_data(form, submitter))
        .finish();
    let target = overriding("formtarget", "target").filter(|t| !t.is_empty());
    if method == "post" {
        return Some(Submission {
            url: url.to_string(),
            post_body: Some(body),
            target,
        });
    }
    url.set_query(Some(&body));
    Some(Submission {
        url: url.to_string(),
        post_body: None,
        target,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmo_engine::renderer::dom::api::get_element_by_id;
    use cosmo_engine::renderer::html::parser::HtmlParser;
    use cosmo_engine::renderer::html::token::HtmlTokenizer;

    fn document(html: &str) -> NodeRef {
        let window = HtmlParser::new(HtmlTokenizer::new(html.to_string())).construct_tree();
        let document = window.borrow().document();
        document
    }

    fn by_id(document: &NodeRef, id: &str) -> NodeRef {
        get_element_by_id(Some(document.clone()), &id.to_string()).expect("element")
    }

    const FORM: &str = r#"<form id=f action="/search?x=1#top" method=get>
        <input name=q value="a b"><input type=password name=p value=secret>
        <input type=checkbox name=c1 checked><input type=checkbox name=c2 value=two>
        <input type=radio name=r value=1><input type=radio name=r value=2 checked>
        <select name=s><option>first<option value=v selected>second</select>
        <textarea name=t>note</textarea>
        <input name=off disabled value=x><input value=unnamed>
        <input type=submit id=go name=b value=Go><input type=submit id=no name=d value=Other>
        </form>"#;

    #[test]
    fn form_data_follows_control_state() {
        let doc = document(FORM);
        let form = by_id(&doc, "f");
        let go = by_id(&doc, "go");
        let data = form_data(&form, Some(&go));
        let expected: Vec<(String, String)> = [
            ("q", "a b"),
            ("p", "secret"),
            ("c1", "on"),
            ("r", "2"),
            ("s", "v"),
            ("t", "note"),
            ("b", "Go"),
        ]
        .iter()
        .map(|(n, v)| (n.to_string(), v.to_string()))
        .collect();
        assert_eq!(data, expected);
    }

    #[test]
    fn get_replaces_the_query_and_drops_the_fragment() {
        let doc = document(FORM);
        let form = by_id(&doc, "f");
        let submission = build_submission(&form, None, "http://x.test/a/b").unwrap();
        assert!(submission.post_body.is_none());
        assert!(
            submission.url.starts_with("http://x.test/search?q=a+b&p=secret"),
            "{}",
            submission.url
        );
        assert!(!submission.url.contains("x=1"), "{}", submission.url);
        assert!(!submission.url.contains('#'), "{}", submission.url);
    }

    #[test]
    fn post_keeps_the_data_out_of_the_url() {
        let doc = document(r#"<form id=f action=/login method=POST><input type=password name=p value=secret></form>"#);
        let form = by_id(&doc, "f");
        let submission = build_submission(&form, None, "http://x.test/").unwrap();
        assert_eq!(submission.url, "http://x.test/login");
        assert_eq!(submission.post_body.as_deref(), Some("p=secret"));
    }

    #[test]
    fn a_submitter_overrides_action_and_method() {
        let doc = document(r#"<form id=f action=/a><button id=b formaction=/c formmethod=post name=n value=v>x</button></form>"#);
        let form = by_id(&doc, "f");
        let button = by_id(&doc, "b");
        let submission = build_submission(&form, Some(&button), "http://x.test/").unwrap();
        assert_eq!(submission.url, "http://x.test/c");
        assert_eq!(submission.post_body.as_deref(), Some("n=v"));
    }

    #[test]
    fn checkbox_toggles_and_radio_clears_its_group() {
        let doc = document(FORM);
        let form = by_id(&doc, "f");
        let mut all = Vec::new();
        descendants(&form, &mut all);
        let checkbox = all.iter().find(|n| attribute(n, "name").as_deref() == Some("c2")).unwrap();
        assert!(matches!(activate(checkbox), Activation::Toggled(_)));
        assert!(attribute(checkbox, "checked").is_some());
        assert!(matches!(activate(checkbox), Activation::Toggled(_)));
        assert!(attribute(checkbox, "checked").is_none());
        let radios: Vec<&NodeRef> = all
            .iter()
            .filter(|n| attribute(n, "name").as_deref() == Some("r"))
            .collect();
        activate(radios[0]);
        assert!(attribute(radios[0], "checked").is_some());
        assert!(attribute(radios[1], "checked").is_none());
    }

    #[test]
    fn a_label_activates_its_control() {
        let doc = document(r#"<label for=c id=l>x</label><input type=checkbox id=c><label id=m><input type=checkbox id=d>y</label>"#);
        activate(&by_id(&doc, "l"));
        assert!(attribute(&by_id(&doc, "c"), "checked").is_some());
        activate(&by_id(&doc, "m"));
        assert!(attribute(&by_id(&doc, "d"), "checked").is_some());
    }

    #[test]
    fn disabled_controls_do_nothing() {
        let doc = document(r#"<fieldset disabled><input type=checkbox id=c></fieldset>"#);
        assert!(matches!(activate(&by_id(&doc, "c")), Activation::None));
        assert!(attribute(&by_id(&doc, "c"), "checked").is_none());
    }

    #[test]
    fn enter_submits_through_the_default_button() {
        let doc = document(r#"<form id=f><input id=i><button type=button>x</button><button id=b>y</button></form>"#);
        match implicit_submission(&by_id(&doc, "i")) {
            Some(Activation::Submit { submitter, .. }) => {
                assert!(Rc::ptr_eq(&submitter.unwrap(), &by_id(&doc, "b")));
            }
            _ => panic!("should submit"),
        }
    }
}
