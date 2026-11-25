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

// ---- the bundle config and the Mac's config ----

fn read_text(name: &str) -> String {
    fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join(name)).unwrap()
}

fn names(problems: &[String], needle: &str) -> bool {
    problems.iter().any(|problem| problem.contains(needle))
}

/// The two programs the bundle takes along, in Tauri's `externalBin` form.
/// The names are the ones `crates/xtask` stages under `binaries/`, with the
/// target triple added. Tauri installs them beside the app as `ffmpeg` and
/// `ffprobe`, where the core looks first.
const SIDECARS: &[&str] = &["binaries/ffmpeg", "binaries/ffprobe"];

/// The command that downloads, checks and stages them before Tauri bundles.
const SIDECAR_BUILD: &str = "cargo run --locked -p video-optimizer-xtask -- ffmpeg";

/// What is wrong with the bundle-only config: `bundle.externalBin` must be
/// exactly [`SIDECARS`] (a list, in that order) and `build.beforeBuildCommand`
/// exactly the string [`SIDECAR_BUILD`]. The sidecars are named here and not
/// in `tauri.conf.json` because tauri-build stops when a listed sidecar is
/// missing, which would break `cargo test` and `clippy` on a fresh checkout.
fn bundle_config_problems(text: &str) -> Vec<String> {
    let config: Value = match serde_json::from_str(text) {
        Ok(config) => config,
        Err(error) => return vec![format!("not JSON: {error}")],
    };
    let mut problems = Vec::new();
    match config.pointer("/bundle/externalBin") {
        Some(found) if *found == json!(SIDECARS) => {}
        Some(found) => problems.push(format!(
            "bundle.externalBin: {found} is not {}",
            json!(SIDECARS)
        )),
        None => problems.push("bundle.externalBin: missing".to_owned()),
    }
    match config.pointer("/build/beforeBuildCommand") {
        Some(Value::String(command)) if command == SIDECAR_BUILD => {}
        Some(other) => problems.push(format!(
            "build.beforeBuildCommand: {other} is not {SIDECAR_BUILD:?}"
        )),
        None => problems.push("build.beforeBuildCommand: missing".to_owned()),
    }
    problems
}

/// What is wrong with a config every build reads: it may not name a sidecar,
/// or `cargo test` stops as soon as `src-tauri/binaries/` has no programs.
fn main_config_problems(config: &Value) -> Vec<String> {
    match config.pointer("/bundle/externalBin") {
        None => Vec::new(),
        Some(found) => vec![format!(
            "bundle.externalBin in a config every build reads: {found}"
        )],
    }
}

/// The platform configs Tauri merges into every build on that platform
/// (`tauri.<platform>.conf.json`, `.json5`, `Tauri.<platform>.toml`). Only
/// `tauri.macos.conf.json` is allowed to exist.
fn platform_configs(dir: &Path) -> Vec<String> {
    let platforms = ["linux", "windows", "macos", "android", "ios"];
    let names = platforms.iter().flat_map(|platform| {
        [
            format!("tauri.{platform}.conf.json"),
            format!("tauri.{platform}.conf.json5"),
            format!("Tauri.{platform}.toml"),
        ]
    });
    names.filter(|name| dir.join(name).exists()).collect()
}

#[test]
fn bundle_config_ships_ffmpeg_and_ffprobe() {
    let text = read_text("tauri.bundle.conf.json");
    assert_eq!(bundle_config_problems(&text), Vec::<String>::new());
    let config: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(
        config["bundle"]["externalBin"],
        json!(["binaries/ffmpeg", "binaries/ffprobe"])
    );
    // Nothing else: the rest comes from tauri.conf.json.
    let mut keys: Vec<&str> = config
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["$schema", "build", "bundle"]);
}

#[test]
fn a_bundle_config_without_ffprobe_or_the_staging_command_is_caught_in_a_fixture() {
    let build = format!("\"build\": {{ \"beforeBuildCommand\": \"{SIDECAR_BUILD}\" }}");
    let bundle = "\"bundle\": { \"externalBin\": [\"binaries/ffmpeg\", \"binaries/ffprobe\"] }";
    let good = format!("{{ {build}, {bundle} }}");
    assert_eq!(bundle_config_problems(&good), Vec::<String>::new());
    let without_ffprobe = "\"bundle\": { \"externalBin\": [\"binaries/ffmpeg\"] }";
    let without_ffmpeg = "\"bundle\": { \"externalBin\": [\"binaries/ffprobe\"] }";
    let extra = "\"bundle\": { \"externalBin\": [\"binaries/ffmpeg\", \"binaries/ffprobe\", \"binaries/x\"] }";
    let other = "\"bundle\": { \"externalBin\": [\"binaries/ffmpeg\", \"binaries/ffplay\"] }";
    let reversed = "\"bundle\": { \"externalBin\": [\"binaries/ffprobe\", \"binaries/ffmpeg\"] }";
    let string = "\"bundle\": { \"externalBin\": \"binaries/ffmpeg\" }";
    let other_build = "\"build\": { \"beforeBuildCommand\": \"cargo build --release\" }";
    let object_build =
        format!("\"build\": {{ \"beforeBuildCommand\": {{ \"script\": \"{SIDECAR_BUILD}\" }} }}");
    for (fixture, needle) in [
        (format!("{{ {build} }}"), "externalBin: missing"),
        (format!("{{ {build}, {without_ffprobe} }}"), "externalBin"),
        (format!("{{ {build}, {without_ffmpeg} }}"), "externalBin"),
        (format!("{{ {build}, {extra} }}"), "externalBin"),
        (format!("{{ {build}, {other} }}"), "externalBin"),
        (format!("{{ {build}, {reversed} }}"), "externalBin"),
        (format!("{{ {build}, {string} }}"), "externalBin"),
        (format!("{{ {bundle} }}"), "beforeBuildCommand: missing"),
        (
            format!("{{ {other_build}, {bundle} }}"),
            "beforeBuildCommand",
        ),
        (
            format!("{{ {object_build}, {bundle} }}"),
            "beforeBuildCommand",
        ),
        (format!("{{ {build}, {bundle}"), "not JSON"),
    ] {
        let problems = bundle_config_problems(&fixture);
        assert!(names(&problems, needle), "{fixture}: {problems:?}");
    }
}

#[test]
fn the_main_config_has_no_external_bin_and_the_only_platform_config_is_the_macos_one() {
    let config = read_json("tauri.conf.json");
    assert_eq!(main_config_problems(&config), Vec::<String>::new());
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert_eq!(platform_configs(dir), ["tauri.macos.conf.json"]);
}

#[test]
fn a_main_config_with_an_external_bin_or_a_stray_platform_config_is_caught_in_a_fixture() {
    for external_bin in [json!(SIDECARS), json!([]), json!("binaries/ffmpeg")] {
        let fixture = json!({ "bundle": { "active": true, "externalBin": external_bin } });
        let problems = main_config_problems(&fixture);
        assert!(names(&problems, "externalBin"), "{fixture}: {problems:?}");
    }
    let folder = tempfile::tempdir().unwrap();
    assert_eq!(platform_configs(folder.path()), Vec::<String>::new());
    fs::write(folder.path().join("tauri.linux.conf.json"), "{}").unwrap();
    fs::write(folder.path().join("Tauri.windows.toml"), "").unwrap();
    assert_eq!(
        platform_configs(folder.path()),
        ["tauri.linux.conf.json", "Tauri.windows.toml"]
    );
}

/// The bundle formats the Mac build ships: the `.app` and the `.dmg` that
/// holds it. Tauri merges `tauri.macos.conf.json` over `tauri.conf.json` on
/// macOS only, so this list replaces `["nsis"]` there.
const MACOS_BUNDLE_TARGETS: &[&str] = &["app", "dmg"];

/// The keys one object may hold, as problems for any other key.
fn unexpected_keys(
    object: &serde_json::Map<String, Value>,
    allowed: &[&str],
    path: &str,
) -> Vec<String> {
    object
        .keys()
        .filter(|key| !allowed.contains(&key.as_str()))
        .map(|key| format!("{path}: {key} is not allowed"))
        .collect()
}

/// What is wrong with `tauri.macos.conf.json`. It may hold only `$schema` and
/// `bundle`; `bundle` only `targets` and `macOS`; `bundle.macOS` only
/// `signingIdentity` and `minimumSystemVersion`. The targets are exactly
/// [`MACOS_BUNDLE_TARGETS`], each once. The identity is the string `-` (an
/// ad-hoc signature: no certificate, no secret) and the minimum version the
/// string `11.0`, the first macOS that runs Apple Silicon. Values are judged
/// by JSON type: the number `11` and `null` are wrong values.
fn macos_config_problems(config: &Value) -> Vec<String> {
    let mut problems = Vec::new();
    if let Some(top) = config.as_object() {
        problems.extend(unexpected_keys(top, &["$schema", "bundle"], "top level"));
    }
    let Some(bundle) = config.get("bundle").and_then(Value::as_object) else {
        problems.push("bundle: missing or not an object".to_owned());
        return problems;
    };
    problems.extend(unexpected_keys(bundle, &["targets", "macOS"], "bundle"));

    match bundle.get("targets") {
        None => problems.push("bundle.targets: no list (Tauri then builds all)".to_owned()),
        Some(targets) => match targets.as_array() {
            None => problems.push(format!("bundle.targets: {targets} is not a list")),
            Some(list) => {
                for target in list {
                    match target.as_str() {
                        Some(name) if MACOS_BUNDLE_TARGETS.contains(&name) => {}
                        Some(name) => problems.push(format!(
                            "bundle.targets: {name} is not one of {MACOS_BUNDLE_TARGETS:?}"
                        )),
                        None => {
                            problems.push(format!("bundle.targets: {target} is not a string"));
                        }
                    }
                }
                for wanted in MACOS_BUNDLE_TARGETS {
                    let times = list.iter().filter(|t| t.as_str() == Some(wanted)).count();
                    if times != 1 {
                        problems.push(format!(
                            "bundle.targets: {wanted} must be listed once, found {times} times"
                        ));
                    }
                }
            }
        },
    }

    let Some(macos) = bundle.get("macOS").and_then(Value::as_object) else {
        problems.push("bundle.macOS: missing or not an object".to_owned());
        return problems;
    };
    problems.extend(unexpected_keys(
        macos,
        &["signingIdentity", "minimumSystemVersion"],
        "bundle.macOS",
    ));
    for (key, wanted) in [("signingIdentity", "-"), ("minimumSystemVersion", "11.0")] {
        if macos.get(key) != Some(&Value::String(wanted.to_owned())) {
            problems.push(format!(
                "bundle.macOS.{key}: must be the string {wanted:?}, found {}",
                macos
                    .get(key)
                    .map_or("nothing".to_owned(), Value::to_string)
            ));
        }
    }
    problems
}

/// A Mac config that passes [`macos_config_problems`], for the fixtures below
/// to change one thing in.
fn good_macos_config() -> Value {
    json!({
        "$schema": "https://schema.tauri.app/config/2",
        "bundle": {
            "targets": ["app", "dmg"],
            "macOS": { "signingIdentity": "-", "minimumSystemVersion": "11.0" }
        }
    })
}

#[test]
fn macos_config_signs_ad_hoc_for_11_0() {
    let config = read_json("tauri.macos.conf.json");
    assert_eq!(macos_config_problems(&config), Vec::<String>::new());
    assert_eq!(config["bundle"]["targets"], json!(MACOS_BUNDLE_TARGETS));
    assert_eq!(config["bundle"]["macOS"]["signingIdentity"], "-");
    assert_eq!(config["bundle"]["macOS"]["minimumSystemVersion"], "11.0");
}

#[test]
fn a_macos_config_with_other_targets_is_caught_in_a_fixture() {
    let with = |targets: Option<Value>| {
        let mut config = good_macos_config();
        match targets {
            Some(targets) => config["bundle"]["targets"] = targets,
            None => {
                config["bundle"].as_object_mut().unwrap().remove("targets");
            }
        }
        config
    };
    for (fixture, needle) in [
        (with(None), "no list"),
        (with(Some(json!("all"))), "not a list"),
        (with(Some(json!(["app"]))), "dmg"),
        (with(Some(json!(["dmg"]))), "app"),
        (with(Some(json!(["app", "dmg", "deb"]))), "deb"),
        (with(Some(json!(["dmg", 1]))), "not a string"),
        (with(Some(json!(["app", "app", "dmg"]))), "app"),
        (with(Some(json!([]))), "app"),
    ] {
        let problems = macos_config_problems(&fixture);
        assert!(names(&problems, needle), "{fixture}: {problems:?}");
    }
    for ok in [json!(["app", "dmg"]), json!(["dmg", "app"])] {
        assert_eq!(macos_config_problems(&with(Some(ok))), Vec::<String>::new());
    }
}

#[test]
fn a_macos_config_without_an_ad_hoc_signature_is_caught_in_a_fixture() {
    let with = |identity: Option<Value>| {
        let mut config = good_macos_config();
        let macos = config["bundle"]["macOS"].as_object_mut().unwrap();
        match identity {
            Some(identity) => macos.insert("signingIdentity".to_owned(), identity),
            None => macos.remove("signingIdentity"),
        };
        config
    };
    for fixture in [
        with(None),
        with(Some(json!(""))),
        with(Some(json!("Developer ID Application: X"))),
        with(Some(Value::Null)),
        with(Some(json!(["-"]))),
    ] {
        let problems = macos_config_problems(&fixture);
        assert!(
            names(&problems, "signingIdentity"),
            "{fixture}: {problems:?}"
        );
    }
    assert_eq!(
        macos_config_problems(&with(Some(json!("-")))),
        Vec::<String>::new()
    );
}

#[test]
fn a_macos_config_with_another_minimum_version_is_caught_in_a_fixture() {
    let with = |version: Option<Value>| {
        let mut config = good_macos_config();
        let macos = config["bundle"]["macOS"].as_object_mut().unwrap();
        match version {
            Some(version) => macos.insert("minimumSystemVersion".to_owned(), version),
            None => macos.remove("minimumSystemVersion"),
        };
        config
    };
    for fixture in [
        with(None),
        with(Some(json!("10.13"))),
        with(Some(json!("11"))),
        with(Some(json!(11))),
        with(Some(json!(11.0))),
    ] {
        let problems = macos_config_problems(&fixture);
        assert!(
            names(&problems, "minimumSystemVersion"),
            "{fixture}: {problems:?}"
        );
    }
    assert_eq!(
        macos_config_problems(&with(Some(json!("11.0")))),
        Vec::<String>::new()
    );
}

#[test]
fn a_macos_config_that_sets_anything_else_is_caught_in_a_fixture() {
    let mut top = good_macos_config();
    top["app"] = json!({ "windows": [] });
    let mut resources = good_macos_config();
    resources["bundle"]["resources"] = json!({ "a": "b" });
    let mut external_bin = good_macos_config();
    external_bin["bundle"]["externalBin"] = json!(SIDECARS);
    let mut entitlements = good_macos_config();
    entitlements["bundle"]["macOS"]["entitlements"] = json!("e.plist");
    let mut bundle_not_object = good_macos_config();
    bundle_not_object["bundle"] = json!(["app", "dmg"]);
    let mut macos_not_object = good_macos_config();
    macos_not_object["bundle"]["macOS"] = json!("-");
    let mut no_macos = good_macos_config();
    no_macos["bundle"].as_object_mut().unwrap().remove("macOS");
    for (fixture, needle) in [
        (top, "app"),
        (resources, "resources"),
        (external_bin, "externalBin"),
        (entitlements, "entitlements"),
        (bundle_not_object, "bundle"),
        (macos_not_object, "bundle.macOS"),
        (no_macos, "bundle.macOS"),
        (json!({}), "bundle"),
    ] {
        let problems = macos_config_problems(&fixture);
        assert!(names(&problems, needle), "{fixture}: {problems:?}");
    }
    // With or without the schema line, the config passes.
    let mut without_schema = good_macos_config();
    without_schema.as_object_mut().unwrap().remove("$schema");
    assert_eq!(macos_config_problems(&without_schema), Vec::<String>::new());
}

// ---- the static C runtime on Windows ----

const WINDOWS_MSVC: &str = "x86_64-pc-windows-msvc";

/// Problems with the Cargo configuration that links the C runtime
/// statically into the Windows build, so that a clean Windows machine
/// without the Visual C++ redistributable can start the app. Read as TOML:
/// `target.x86_64-pc-windows-msvc.rustflags` must hold `-C` followed by
/// `target-feature=+crt-static`, or the joined `-Ctarget-feature=+crt-static`.
fn crt_static_problems(config_toml: &str) -> Vec<String> {
    let table = match config_toml.parse::<toml::Table>() {
        Ok(table) => table,
        Err(error) => return vec![format!("not TOML: {error}")],
    };
    let Some(target) = table
        .get("target")
        .and_then(toml::Value::as_table)
        .and_then(|targets| targets.get(WINDOWS_MSVC))
        .and_then(toml::Value::as_table)
    else {
        return vec![format!("no [target.{WINDOWS_MSVC}] table")];
    };
    let Some(flags) = target.get("rustflags").and_then(toml::Value::as_array) else {
        return vec!["no rustflags array".to_owned()];
    };
    let flags: Vec<&str> = flags.iter().filter_map(toml::Value::as_str).collect();
    let static_crt = flags.contains(&"-Ctarget-feature=+crt-static")
        || flags
            .windows(2)
            .any(|pair| pair == ["-C", "target-feature=+crt-static"]);
    if static_crt {
        Vec::new()
    } else {
        vec![format!(
            "rustflags {flags:?} do not link the CRT statically"
        )]
    }
}

#[test]
fn the_windows_build_links_the_c_runtime_statically() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../.cargo/config.toml");
    let config = fs::read_to_string(path).unwrap();
    assert_eq!(crt_static_problems(&config), Vec::<String>::new());
}

#[test]
fn a_cargo_config_with_the_static_runtime_in_either_spelling_passes_a_fixture() {
    for flags in [
        r#"["-C", "target-feature=+crt-static"]"#,
        r#"["-Ctarget-feature=+crt-static"]"#,
        r#"["-C", "link-arg=/X", "-C", "target-feature=+crt-static"]"#,
    ] {
        let config = format!("[target.{WINDOWS_MSVC}]\nrustflags = {flags}\n");
        assert_eq!(
            crt_static_problems(&config),
            Vec::<String>::new(),
            "{config}"
        );
    }
}

#[test]
fn a_cargo_config_without_the_static_runtime_is_caught_in_a_fixture() {
    let configs = [
        // Only in a comment.
        format!(
            "[target.{WINDOWS_MSVC}]\n# rustflags = [\"-C\", \"target-feature=+crt-static\"]\n"
        ),
        format!("[target.{WINDOWS_MSVC}]\nrustflags = [] # -C target-feature=+crt-static\n"),
        // For another target.
        "[target.x86_64-pc-windows-gnu]\nrustflags = [\"-C\", \"target-feature=+crt-static\"]\n"
            .to_owned(),
        // The flag without its `-C`, the pair the wrong way round, or another feature.
        format!("[target.{WINDOWS_MSVC}]\nrustflags = [\"target-feature=+crt-static\"]\n"),
        format!("[target.{WINDOWS_MSVC}]\nrustflags = [\"target-feature=+crt-static\", \"-C\"]\n"),
        format!("[target.{WINDOWS_MSVC}]\nrustflags = [\"-C\", \"target-feature=-crt-static\"]\n"),
        // A string where an array belongs, and no rustflags at all.
        format!("[target.{WINDOWS_MSVC}]\nrustflags = \"-C target-feature=+crt-static\"\n"),
        format!("[target.{WINDOWS_MSVC}]\nlinker = \"link.exe\"\n"),
        // Not TOML.
        format!("[target.{WINDOWS_MSVC}\nrustflags = [\"-C\", \"target-feature=+crt-static\"]\n"),
    ];
    for config in &configs {
        assert_eq!(crt_static_problems(config).len(), 1, "{config}");
    }
}
