//! Guards on the window's settings, so a later change cannot widen them
//! silently: the capability grants the six app commands and event listening
//! only, the content security policy stays strict (only `media-src` names the
//! `media` scheme), and the window and bundle are as the plan sets them.
//!
//! Each checker is also run on a fixture that breaks it, so a checker that
//! reports nothing cannot pass for one that works.

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

/// Every permission the window has: the six commands, listening and
/// unlistening. Nothing else.
const ALLOWED: &[&str] = &[
    "allow-choose-video",
    "allow-choose-folder",
    "allow-clear-folder",
    "allow-make-sample",
    "allow-save",
    "allow-cancel-save",
    "core:event:allow-listen",
    "core:event:allow-unlisten",
];

fn read_json(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

// ---- the capability ----

/// What is wrong with a capability: a permission that is not in [`ALLOWED`]
/// (an object with a scope counts as not allowed, as none of ours needs one),
/// one of [`ALLOWED`] missing or given twice, a window other than `main`.
fn capability_problems(capability: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let Some(permissions) = capability["permissions"].as_array() else {
        return vec!["no permissions list".to_owned()];
    };
    for permission in permissions {
        if !permission.as_str().is_some_and(|p| ALLOWED.contains(&p)) {
            problems.push(format!("unexpected {permission}"));
        }
    }
    for allowed in ALLOWED {
        match permissions
            .iter()
            .filter(|p| p.as_str() == Some(allowed))
            .count()
        {
            1 => {}
            0 => problems.push(format!("missing {allowed}")),
            _ => problems.push(format!("{allowed} given more than once")),
        }
    }
    if capability["windows"] != json!(["main"]) {
        problems.push(format!(
            "windows is {}, not [\"main\"]",
            capability["windows"]
        ));
    }
    problems
}

#[test]
fn capability_grants_exactly_the_six_commands_and_listen() {
    let capability = read_json("capabilities/default.json");
    assert_eq!(capability_problems(&capability), Vec::<String>::new());
    let commands = capability["permissions"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|p| p.as_str().is_some_and(|p| p.starts_with("allow-")))
        .count();
    assert_eq!(commands, 6);
}

#[test]
fn capability_refuses_an_extra_permission() {
    let mut capability = read_json("capabilities/default.json");
    let permissions = capability["permissions"].as_array_mut().unwrap();
    permissions.push(json!("dialog:default"));
    assert_eq!(
        capability_problems(&capability),
        ["unexpected \"dialog:default\""]
    );
}

#[test]
fn capability_refuses_a_scoped_permission_a_missing_one_and_a_second_window() {
    let mut capability = read_json("capabilities/default.json");
    let permissions = capability["permissions"].as_array_mut().unwrap();
    assert!(!permissions.is_empty());
    permissions.retain(|p| p != "allow-save");
    permissions.push(json!({ "identifier": "core:event:allow-listen", "allow": [] }));
    capability["windows"] = json!(["main", "other"]);
    let problems = capability_problems(&capability);
    assert!(
        problems.contains(&"missing allow-save".to_owned()),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.starts_with("unexpected {")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.starts_with("windows is")),
        "{problems:?}"
    );
}

/// The permissions the commands declared in a `build.rs` text need
/// (`allow-<command>`, sorted): the quoted names inside its
/// `.commands(&[ … ])` list. Empty when there is no such list.
fn declared_permissions(build_rs: &str) -> Vec<String> {
    let Some(start) = build_rs.find(".commands(&[") else {
        return Vec::new();
    };
    let list = &build_rs[start..];
    let list = &list[..list.find("])").unwrap_or(list.len())];
    let mut declared: Vec<String> = list
        .split('"')
        .skip(1)
        .step_by(2)
        .map(|name| format!("allow-{}", name.replace('_', "-")))
        .collect();
    declared.sort();
    declared
}

/// The `allow-` permissions of [`ALLOWED`], sorted.
fn command_permissions() -> Vec<String> {
    let mut expected: Vec<String> = ALLOWED
        .iter()
        .filter(|p| p.starts_with("allow-"))
        .map(|p| p.to_string())
        .collect();
    expected.sort();
    expected
}

#[test]
fn build_rs_declares_the_same_six_commands() {
    // The commands `build.rs` declares are the ones that need a permission;
    // a command missing there would be open to the window.
    let build = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("build.rs")).unwrap();
    let declared = declared_permissions(&build);
    assert_eq!(declared.len(), 6, "{declared:?}");
    assert_eq!(declared, command_permissions());
}

#[test]
fn a_build_rs_missing_a_command_is_caught_in_a_fixture() {
    let fixture = r#"AppManifest::new().commands(&["choose_video", "choose_folder",
        "clear_folder", "make_sample", "cancel_save"]);"#;
    let declared = declared_permissions(fixture);
    assert_eq!(declared.len(), 5, "{declared:?}");
    assert_ne!(declared, command_permissions());
    assert!(declared_permissions("fn main() {}").is_empty());
}

// ---- the content security policy ----

/// The policy as it must be, directive by directive.
const EXPECTED_CSP: &[(&str, &[&str])] = &[
    ("default-src", &["'self'"]),
    ("script-src", &["'self'"]),
    ("style-src", &["'self'"]),
    ("img-src", &["'self'"]),
    ("media-src", &["media:", "http://media.localhost"]),
    ("font-src", &["'self'"]),
    ("connect-src", &["ipc:", "http://ipc.localhost"]),
    ("object-src", &["'none'"]),
    ("base-uri", &["'none'"]),
    ("form-action", &["'none'"]),
    ("frame-src", &["'none'"]),
];

/// The policy split into directives: the name in lower case, then its
/// sources, in order. Empty directives (`;;`) are skipped.
fn directives(csp: &str) -> Vec<(String, Vec<String>)> {
    csp.split(';')
        .map(|directive| directive.split_whitespace().collect::<Vec<_>>())
        .filter(|tokens| !tokens.is_empty())
        .map(|tokens| {
            (
                tokens[0].to_ascii_lowercase(),
                tokens[1..].iter().map(|s| s.to_string()).collect(),
            )
        })
        .collect()
}

/// What makes a policy weaker or other than [`EXPECTED_CSP`]: any
/// `'unsafe-…'` source, a wildcard, `data:` or `blob:` anywhere; a directive
/// missing, given twice or not expected; a directive with other sources than
/// expected (compared without case, in any order).
fn csp_problems(csp: &str) -> Vec<String> {
    let mut problems = Vec::new();
    let directives = directives(csp);
    for (name, sources) in &directives {
        for source in sources {
            let lower = source.to_ascii_lowercase();
            if lower.starts_with("'unsafe-")
                || lower.contains('*')
                || lower == "data:"
                || lower == "blob:"
            {
                problems.push(format!("{name} {source}"));
            }
        }
        if !EXPECTED_CSP.iter().any(|(expected, _)| expected == name) {
            problems.push(format!("unexpected directive {name}"));
        }
    }
    for (name, expected) in EXPECTED_CSP {
        let given: Vec<Vec<String>> = directives
            .iter()
            .filter(|(directive, _)| directive == name)
            .map(|(_, sources)| {
                let mut sources: Vec<String> =
                    sources.iter().map(|s| s.to_ascii_lowercase()).collect();
                sources.sort();
                sources
            })
            .collect();
        let mut expected: Vec<String> = expected.iter().map(|s| s.to_string()).collect();
        expected.sort();
        match given.as_slice() {
            [] => problems.push(format!("missing {name}")),
            [sources] if *sources == expected => {}
            [sources] => problems.push(format!("{name} is {sources:?}, not {expected:?}")),
            _ => problems.push(format!("{name} given more than once")),
        }
    }
    problems
}

fn configured_csp() -> String {
    read_json("tauri.conf.json")["app"]["security"]["csp"]
        .as_str()
        .expect("the CSP is a string")
        .to_owned()
}

#[test]
fn csp_allows_media_only_from_the_media_scheme() {
    let csp = configured_csp();
    assert_eq!(csp_problems(&csp), Vec::<String>::new());
    let parsed = directives(&csp);
    let media: Vec<&Vec<String>> = parsed
        .iter()
        .filter(|(name, _)| name == "media-src")
        .map(|(_, sources)| sources)
        .collect();
    assert_eq!(media.len(), 1, "{parsed:?}");
    assert_eq!(*media[0], ["media:", "http://media.localhost"]);
    // Only media-src names the media scheme.
    for (name, sources) in &parsed {
        if name != "media-src" {
            assert!(
                !sources.iter().any(|s| s.contains("media")),
                "{name} {sources:?}"
            );
        }
    }
}

#[test]
fn csp_is_exactly_the_planned_string() {
    assert_eq!(
        configured_csp(),
        "default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; \
         media-src media: http://media.localhost; font-src 'self'; \
         connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; \
         form-action 'none'; frame-src 'none'"
    );
}

#[test]
fn a_csp_with_media_src_star_is_caught_in_a_fixture() {
    let csp = configured_csp().replace("media-src media: http://media.localhost", "media-src *");
    assert!(csp.contains("media-src *"), "the fixture was built");
    let problems = csp_problems(&csp);
    assert!(problems.contains(&"media-src *".to_owned()), "{problems:?}");
}

#[test]
fn a_csp_with_unsafe_sources_or_a_changed_directive_is_caught_in_a_fixture() {
    let good = configured_csp();
    for (from, to, wanted) in [
        (
            "script-src 'self'",
            "script-src 'self' 'unsafe-eval'",
            "script-src 'unsafe-eval'",
        ),
        (
            "style-src 'self'",
            "style-src 'self' 'unsafe-inline'",
            "style-src 'unsafe-inline'",
        ),
        ("img-src 'self'", "img-src 'self' data:", "img-src data:"),
        (
            "media-src media: http://media.localhost",
            "media-src media: http://media.localhost blob:",
            "media-src blob:",
        ),
    ] {
        let csp = good.replace(from, to);
        assert_ne!(csp, good, "the fixture changed {from}");
        let problems = csp_problems(&csp);
        assert!(problems.contains(&wanted.to_owned()), "{to}: {problems:?}");
    }
    let without_media = good.replace("media-src media: http://media.localhost; ", "");
    assert!(csp_problems(&without_media).contains(&"missing media-src".to_owned()));
    let remote = good.replace("img-src 'self'", "img-src 'self' https://example.com");
    assert!(
        csp_problems(&remote)
            .iter()
            .any(|p| p.starts_with("img-src is")),
        "a remote source changes the directive"
    );
    let extra = format!("{good}; worker-src 'self'");
    assert!(csp_problems(&extra).contains(&"unexpected directive worker-src".to_owned()));
}

// ---- the window and the bundle ----

/// What is wrong with `tauri.conf.json` outside the CSP: the names, the one
/// window and its size, native drag and drop, the global API, the page
/// folder, and the bundle targets (exactly `["nsis"]`: no Linux or MSI
/// target; the Mac's come from `tauri.macos.conf.json`).
fn config_problems(config: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    let mut expect = |path: &str, actual: &Value, wanted: Value| {
        if *actual != wanted {
            problems.push(format!("{path} is {actual}, not {wanted}"));
        }
    };
    expect(
        "productName",
        &config["productName"],
        json!("Video optimizer"),
    );
    expect(
        "identifier",
        &config["identifier"],
        json!("io.github.aureuslunae.videooptimizer"),
    );
    // The version comes from Cargo.toml.
    expect("version", &config["version"], Value::Null);
    expect(
        "frontendDist",
        &config["build"]["frontendDist"],
        json!("../ui"),
    );
    expect(
        "withGlobalTauri",
        &config["app"]["withGlobalTauri"],
        json!(true),
    );
    expect(
        "bundle.targets",
        &config["bundle"]["targets"],
        json!(["nsis"]),
    );
    expect(
        "bundle.externalBin",
        &config["bundle"]["externalBin"],
        Value::Null,
    );
    let windows = config["app"]["windows"].as_array();
    match windows.map(Vec::as_slice) {
        Some([window]) => {
            for (key, wanted) in [
                ("label", json!("main")),
                ("title", json!("Video optimizer")),
                ("width", json!(1100)),
                ("height", json!(720)),
                ("minWidth", json!(640)),
                ("minHeight", json!(480)),
                // `false` would hand a drop to the page, paths and all.
                ("dragDropEnabled", json!(true)),
            ] {
                expect(&format!("window.{key}"), &window[key], wanted);
            }
        }
        _ => problems.push("not exactly one window".to_owned()),
    }
    problems
}

#[test]
fn the_config_names_the_app_and_its_one_window() {
    assert_eq!(
        config_problems(&read_json("tauri.conf.json")),
        Vec::<String>::new()
    );
}

#[test]
fn a_config_with_another_target_or_window_is_caught_in_a_fixture() {
    let good = read_json("tauri.conf.json");

    let mut targets = good.clone();
    targets["bundle"]["targets"] = json!(["nsis", "deb"]);
    let problems = config_problems(&targets);
    assert!(
        problems.iter().any(|p| p.starts_with("bundle.targets")),
        "{problems:?}"
    );

    let mut all = good.clone();
    all["bundle"]["targets"] = json!("all");
    assert!(!config_problems(&all).is_empty());

    let mut drop_to_page = good.clone();
    drop_to_page["app"]["windows"][0]["dragDropEnabled"] = json!(false);
    let problems = config_problems(&drop_to_page);
    assert!(
        problems
            .iter()
            .any(|p| p.starts_with("window.dragDropEnabled")),
        "{problems:?}"
    );

    let mut two = good.clone();
    let window = two["app"]["windows"][0].clone();
    two["app"]["windows"].as_array_mut().unwrap().push(window);
    assert!(config_problems(&two).contains(&"not exactly one window".to_owned()));

    let mut renamed = good;
    renamed["productName"] = json!("Optimizer");
    renamed["bundle"]["externalBin"] = json!(["binaries/ffmpeg"]);
    let problems = config_problems(&renamed);
    assert!(
        problems.iter().any(|p| p.starts_with("productName")),
        "{problems:?}"
    );
    assert!(
        problems.iter().any(|p| p.starts_with("bundle.externalBin")),
        "{problems:?}"
    );
}

#[test]
fn the_page_folder_has_an_index_page() {
    let index = Path::new(env!("CARGO_MANIFEST_DIR")).join("../ui/index.html");
    let html = fs::read_to_string(index).expect("ui/index.html exists");
    assert!(html.contains("<title>Video optimizer</title>"), "{html}");
}
