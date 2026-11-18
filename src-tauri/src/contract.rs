//! `src/chrome.js` held against the Rust side it talks to: the commands it invokes, the
//! `Identity` and `TabPayload` fields it reads, and the events it listens for. A mismatch compiles
//! on both sides and fails silently in the webview, so these tests are the only thing that sees it.

use crate::commands::{Identity, TabPayload};
use crate::events::ChromeEvent;
use std::collections::BTreeSet;

const CHROME_JS: &str = include_str!("../../src/chrome.js");
const LIB_RS: &str = include_str!("lib.rs");

fn is_ident(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Every `<object>.<field>` read in `src`, where `object` stands alone (not the tail of a longer
/// name).
fn field_reads(src: &str, object: &str) -> BTreeSet<String> {
    let needle = format!("{object}.");
    let mut out = BTreeSet::new();
    for (at, _) in src.match_indices(&needle) {
        if src[..at].chars().next_back().is_some_and(is_ident) {
            continue;
        }
        let field: String = src[at + needle.len()..]
            .chars()
            .take_while(|&c| is_ident(c))
            .collect();
        if !field.is_empty() {
            out.insert(field);
        }
    }
    out
}

/// The first string-literal argument of every `call("...")` in `src`.
fn literal_args(src: &str, call: &str) -> BTreeSet<String> {
    let needle = format!("{call}(\"");
    src.match_indices(&needle)
        .filter(|(at, _)| !src[..*at].chars().next_back().is_some_and(is_ident))
        .filter_map(|(at, _)| {
            let rest = &src[at + needle.len()..];
            rest.find('"').map(|end| rest[..end].to_string())
        })
        .collect()
}

/// Every double-quoted snake_case literal in `src` (a word with an underscore). In `chrome.js`
/// those are exactly the command names, including the ones passed through a table or a ternary
/// rather than straight to `invoke(...)`.
fn snake_literals(src: &str) -> BTreeSet<String> {
    src.split('"')
        .skip(1)
        .step_by(2)
        .filter(|s| s.contains('_') && s.chars().all(|c| c.is_ascii_lowercase() || c == '_'))
        .map(str::to_string)
        .collect()
}

/// The command names `run()` registers with `generate_handler!`.
fn registered_commands() -> BTreeSet<String> {
    let start = LIB_RS
        .find("generate_handler![")
        .expect("lib.rs registers its commands with generate_handler!");
    let body = &LIB_RS[start..];
    let body = &body[..body.find(']').expect("generate_handler! closes")];
    body.split([',', '['])
        .filter_map(|item| item.trim().strip_prefix("commands::"))
        .map(str::to_string)
        .collect()
}

fn serialized_keys<T: serde::Serialize>(value: &T) -> BTreeSet<String> {
    match serde_json::to_value(value).expect("serializes") {
        serde_json::Value::Object(map) => map.keys().cloned().collect(),
        other => panic!("expected an object, got {other}"),
    }
}

fn missing(read: &BTreeSet<String>, have: &BTreeSet<String>) -> Vec<String> {
    read.difference(have).cloned().collect()
}

#[test]
fn every_identity_field_chrome_js_reads_is_serialized() {
    let identity = Identity {
        title: String::new(),
        colour: None,
        density: Default::default(),
        sidebar_drag: true,
        auto_update: true,
        default_width: 0.0,
    };
    let read = field_reads(CHROME_JS, "id");
    assert!(read.contains("title"), "the scan found no id.<field> reads");
    let gone = missing(&read, &serialized_keys(&identity));
    assert!(
        gone.is_empty(),
        "chrome.js reads id.{gone:?}, which Identity never sends"
    );
}

#[test]
fn every_tab_field_chrome_js_reads_is_serialized() {
    let tab = TabPayload {
        label: String::new(),
        title: String::new(),
        group: None,
        loaded: false,
        active: false,
        detached: false,
        tree: false,
        tree_path: Vec::new(),
    };
    let read = field_reads(CHROME_JS, "t");
    assert!(read.contains("label"), "the scan found no t.<field> reads");
    let gone = missing(&read, &serialized_keys(&tab));
    assert!(
        gone.is_empty(),
        "chrome.js reads t.{gone:?}, which TabPayload never sends"
    );
}

#[test]
fn every_command_chrome_js_names_is_registered() {
    let named = snake_literals(CHROME_JS);
    assert!(
        named.contains("get_tabs"),
        "the scan found no command names in chrome.js"
    );
    let gone = missing(&named, &registered_commands());
    assert!(
        gone.is_empty(),
        "chrome.js invokes {gone:?}, which generate_handler! never registers"
    );
}

#[test]
fn every_event_chrome_js_listens_for_is_emitted() {
    let heard = literal_args(CHROME_JS, "listen");
    let emitted: BTreeSet<String> = ChromeEvent::ALL
        .iter()
        .map(|e| e.name().to_string())
        .collect();
    let gone = missing(&heard, &emitted);
    assert!(
        gone.is_empty(),
        "chrome.js listens for {gone:?}, which nothing emits"
    );
}

#[test]
fn every_event_emitted_to_the_chrome_has_a_chrome_js_listener() {
    let heard = literal_args(CHROME_JS, "listen");
    let deaf: Vec<&str> = ChromeEvent::ALL
        .iter()
        .map(|e| e.name())
        .filter(|name| !heard.contains(*name))
        .collect();
    assert!(
        deaf.is_empty(),
        "{deaf:?} is emitted to the chrome, but chrome.js never listens for it"
    );
}
