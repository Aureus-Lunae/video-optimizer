//! Guards on the build workflows: `.github/workflows/windows.yml` and
//! `.github/workflows/macos.yml`. They cannot be run here, and they build the
//! installers people download, so the rules that keep them safe are checked in
//! a test: every action pinned to a commit, read-only permissions, no
//! secrets, no way to start one from a pull request, the test and build
//! commands present, ffmpeg staged for the tests, the installers checked
//! before they are uploaded, and each uploaded artifact named, filled and kept
//! as agreed. Copied from Lunaepix's guard and adapted: this app has no NASM,
//! no Homebrew formula and no worker sidecar, but it bundles ffmpeg and
//! ffprobe, and its Mac build has two installer jobs (Apple Silicon and Intel).
//!
//! The rules are the same for both files; what differs per platform (the
//! file, the runners, the installer jobs with their artifacts, the cache, the
//! Rust-test policy) is one `Platform` profile that every check reads. The Mac
//! cache steps save only from `main`. The Windows installer job checks that
//! ffmpeg and ffprobe sit beside the app. Each Mac installer job runs
//! `scripts/check-mac-app.sh` with its own architecture on the built app and
//! inside the mounted disk image; the guard here proves the call is there and
//! cannot pass when the script fails, and `check_mac_app.rs` proves what the
//! script refuses. Each check has fixtures it must catch, so it is known to
//! fail when it should.
//!
//! The workflow is parsed as YAML 1.2 (so the key `on` is a name, not
//! boolean true) and every check reads the parsed tree, so quoting, spacing
//! and flow or block style cannot hide anything. Only a `run:` string is
//! scanned as text, and then, for the checks that look for a command, all of
//! it minus its shell comments: they can lose text, which makes them fail,
//! never pass. The release scan reads every word, comments included: a false
//! hit from a comment is accepted, a missed release is not.

use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

use yaml_rust2::{Yaml, YamlLoader};

/// Whether an Installer job restores a build cache.
enum BuildCache {
    /// No cache: the test job's cache is a debug-profile cache, which a
    /// release build cannot use, and one of its own would only add upload time.
    Forbidden,
    /// Its own `Swatinem/rust-cache`, before its first cargo step: Mac minutes
    /// cost more than the cache upload, and the cache also keeps the installed
    /// Tauri command line.
    Required,
}

/// One Installer job: the job that bundles the app and uploads the result.
struct Installer {
    /// The job's id in the workflow.
    job: &'static str,
    /// The exact `runs-on` of the job.
    runner: &'static str,
    /// The `name` of the uploaded artifact.
    artifact: &'static str,
    /// The `path` lines of the upload, in any order.
    artifact_paths: &'static [&'static str],
    /// The architecture `lipo -archs` must give for ffmpeg and ffprobe in the
    /// app (a Mac job); `None`: a Windows job, whose check step looks for the
    /// programs beside the app.
    arch: Option<&'static str>,
}

/// What differs between the platforms' workflow files. Every check reads the
/// profile it is given, so the same rules guard each file.
struct Platform {
    /// The file in `.github/workflows/`.
    file: &'static str,
    /// The exact `runs-on` of the test job.
    test_runner: &'static str,
    /// Every job but `test`.
    installers: &'static [Installer],
    build_cache: BuildCache,
    /// The `save-if` every `Swatinem/rust-cache` step must carry, as the
    /// expression text; `None`: no requirement.
    cache_save_if: Option<&'static str>,
}

/// The only `save-if` the Mac cache steps may have: tag runs cannot restore
/// each other's cache, only `main`'s, so only `main` saves one.
const SAVE_ONLY_FROM_MAIN: &str = "${{ github.ref == 'refs/heads/main' }}";

const WINDOWS: Platform = Platform {
    file: "windows.yml",
    test_runner: "windows-latest",
    installers: &[Installer {
        job: "build",
        runner: "windows-latest",
        artifact: "video-optimizer-windows-installer",
        artifact_paths: &["target/release/bundle/nsis/*.exe"],
        arch: None,
    }],
    build_cache: BuildCache::Forbidden,
    cache_save_if: None,
};

/// Apple Silicon (`macos-latest`) and Intel (`macos-15-intel`); the artifacts
/// are the disk images.
const MACOS: Platform = Platform {
    file: "macos.yml",
    test_runner: "macos-latest",
    installers: &[
        Installer {
            job: "build",
            runner: "macos-latest",
            artifact: "video-optimizer-macos-arm64-dmg",
            artifact_paths: &["target/release/bundle/dmg/*.dmg"],
            arch: Some("arm64"),
        },
        Installer {
            job: "build-intel",
            runner: "macos-15-intel",
            artifact: "video-optimizer-macos-intel-dmg",
            artifact_paths: &["target/release/bundle/dmg/*.dmg"],
            arch: Some("x86_64"),
        },
    ],
    build_cache: BuildCache::Required,
    cache_save_if: Some(SAVE_ONLY_FROM_MAIN),
};

fn workflow(platform: &Platform) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../.github/workflows")
        .join(platform.file);
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

// ---- reading the tree ----

/// The workflow as one YAML mapping, or why it is not.
fn parse(text: &str) -> Result<Yaml, String> {
    let mut docs = YamlLoader::load_from_str(text)
        .map_err(|e| format!("the workflow does not parse as YAML: {e}"))?;
    match (
        docs.len(),
        docs.first().is_some_and(|d| d.as_hash().is_some()),
    ) {
        (1, true) => Ok(docs.remove(0)),
        (1, false) => Err("the workflow is not a YAML mapping".to_owned()),
        (n, _) => Err(format!("the workflow holds {n} YAML documents, not 1")),
    }
}

/// Runs a check on the parsed workflow; one that does not parse is the one problem.
fn on_tree(text: &str, check: impl FnOnce(&Yaml) -> Vec<String>) -> Vec<String> {
    parse(text).map_or_else(|problem| vec![problem], |doc| check(&doc))
}

/// A scalar as text, however written (`22`, `'22'` and `"22"` are all "22").
fn scalar(node: &Yaml) -> Option<String> {
    match node {
        Yaml::String(text) | Yaml::Real(text) => Some(text.clone()),
        Yaml::Integer(number) => Some(number.to_string()),
        Yaml::Boolean(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// Every scalar of the tree, keys and values, at any depth.
fn all_texts(node: &Yaml) -> Vec<String> {
    match node {
        Yaml::Hash(map) => map
            .iter()
            .flat_map(|(k, v)| [all_texts(k), all_texts(v)].concat())
            .collect(),
        Yaml::Array(items) => items.iter().flat_map(all_texts).collect(),
        other => scalar(other).into_iter().collect(),
    }
}

fn jobs(doc: &Yaml) -> Vec<(String, &Yaml)> {
    let map = doc["jobs"].as_hash().into_iter().flatten();
    map.map(|(name, job)| (scalar(name).unwrap_or_default(), job))
        .collect()
}

fn steps_of(job: &Yaml) -> Vec<&Yaml> {
    job["steps"].as_vec().into_iter().flatten().collect()
}

fn all_steps(doc: &Yaml) -> Vec<&Yaml> {
    jobs(doc)
        .into_iter()
        .flat_map(|(_, job)| steps_of(job))
        .collect()
}

/// The `uses` of a step or job as text; one that is not a plain scalar is
/// shown as what it is, so it fails the pin check.
fn uses_of(node: &Yaml) -> Option<String> {
    let uses = &node["uses"];
    (!uses.is_badvalue()).then(|| scalar(uses).unwrap_or_else(|| format!("{uses:?}")))
}

fn uploads_artifact(step: &Yaml) -> bool {
    uses_of(step).is_some_and(|uses| uses.starts_with("actions/upload-artifact@"))
}

/// Every action reference: the `uses` of each step, and of each job that calls
/// a reusable workflow. A `uses` inside `with` is an input, not an action.
fn references(doc: &Yaml) -> Vec<String> {
    let nodes = jobs(doc)
        .into_iter()
        .flat_map(|(_, job)| [vec![job], steps_of(job)].concat());
    nodes.filter_map(uses_of).collect()
}

/// The whole `run` string of every step, comments included.
fn scripts(doc: &Yaml) -> Vec<String> {
    all_steps(doc)
        .into_iter()
        .filter_map(|step| scalar(&step["run"]))
        .collect()
}

fn action_references(text: &str) -> Vec<String> {
    parse(text).map(|doc| references(&doc)).unwrap_or_default()
}

fn run_scripts(text: &str) -> Vec<String> {
    parse(text).map(|doc| scripts(&doc)).unwrap_or_default()
}

// ---- pinned actions ----

fn is_pinned(reference: &str) -> bool {
    let sha = |s: &str| s.len() == 40 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'));
    reference
        .split_once('@')
        .is_some_and(|(name, hash)| name.contains('/') && sha(hash))
}

/// Whether `line` has a `# ...` comment with text right after `reference`
/// (a closing quote may sit in between).
fn has_release_comment(line: &str, reference: &str) -> bool {
    let rest = line.split_once(reference).map_or("", |(_, rest)| rest);
    let rest = rest.trim_start_matches(|c: char| c == '\'' || c == '"' || c.is_whitespace());
    rest.strip_prefix('#')
        .is_some_and(|comment| !comment.trim().is_empty())
}

/// Every `uses` must name `owner/repo@<40 hex digits>` (a tag or branch can
/// be moved to other code; a commit cannot) and its line must end in a
/// `# ...` comment naming the release. The comment is the one thing the tree
/// does not hold, so it is looked up on the `uses:` lines that name the pin.
fn unpinned_actions(text: &str) -> Vec<String> {
    on_tree(text, |doc| {
        let refs = references(doc);
        let (pinned, loose): (Vec<&String>, Vec<&String>) = refs.iter().partition(|r| is_pinned(r));
        let mut problems: Vec<String> = loose
            .iter()
            .map(|r| format!("not pinned to a commit: {r}"))
            .collect();
        let key_of = |line: &&str| {
            line.trim_start()
                .trim_start_matches('-')
                .trim_start()
                .starts_with("uses:")
        };
        let uses_lines: Vec<&str> = text.lines().filter(key_of).collect();
        for pin in pinned.iter().collect::<BTreeSet<_>>() {
            let lines: Vec<&&str> = uses_lines
                .iter()
                .filter(|line| line.contains(pin.as_str()))
                .collect();
            let commented = lines
                .iter()
                .filter(|line| has_release_comment(line, pin))
                .count();
            let used = refs.iter().filter(|r| r == pin).count();
            if lines.len() != used || commented != used {
                problems.push(format!("no release comment: {pin} is used {used}x, {commented} of {} `uses:` lines carry one", lines.len()));
            }
        }
        problems
    })
}

// ---- permissions, secrets, triggers ----

/// Whether a `permissions` node gives `write` or `write-all` anywhere.
fn grants_write(node: &Yaml) -> bool {
    all_texts(node)
        .iter()
        .any(|v| v.eq_ignore_ascii_case("write") || v.eq_ignore_ascii_case("write-all"))
}

/// The top-level block must be exactly `contents: read`, and no `write`
/// permission may appear at the top or in a job. Values are compared as
/// parsed, so `write`, `'write'` and `"write"` are the same.
fn permission_problems(text: &str) -> Vec<String> {
    on_tree(text, |doc| {
        let top = &doc["permissions"];
        let mut problems = Vec::new();
        if top.is_badvalue() {
            problems.push("no top-level permissions block".to_owned());
        } else {
            let read_only = top.as_hash().is_some_and(|map| map.len() == 1)
                && scalar(&top["contents"]).as_deref() == Some("read");
            if !read_only {
                problems.push(format!("top-level permissions are {top:?}"));
            }
            if grants_write(top) {
                problems.push("write permission at the top level".to_owned());
            }
        }
        let writers = jobs(doc)
            .into_iter()
            .filter(|(_, job)| grants_write(&job["permissions"]));
        problems.extend(writers.map(|(name, _)| format!("write permission in job {name}")));
        problems
    })
}

/// Every key or value that reaches a repository secret or the job token: the
/// word `secrets` (`secrets.X`, `secrets['X']`, `toJSON(secrets)`,
/// `secrets: inherit`), `github.token`, `GITHUB_TOKEN`, a `github-token` input.
fn secret_references(text: &str) -> Vec<String> {
    on_tree(text, |doc| {
        let mentions = |value: &String| {
            let lower = value.to_ascii_lowercase();
            // `secrets` as a whole word: not `mysecretsauce`, not `secrets_manager`.
            let word = lower
                .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .any(|w| w == "secrets");
            word || lower.contains("github.token")
                || lower.replace('-', "_").contains("github_token")
        };
        all_texts(doc).into_iter().filter(mentions).collect()
    })
}

/// `on` must hold `push` and `workflow_dispatch` and nothing else, and `push`
/// must filter on the tag `v*` only (no branches or paths, so no push to a
/// branch starts it). A pull request trigger is therefore a problem,
/// `pull_request_target` above all: it runs with a token that can write, on
/// code from a fork. `on: push`, `on: [push]` and `on: {push: ...}` are read alike.
fn trigger_problems(text: &str) -> Vec<String> {
    on_tree(text, |doc| {
        let on = &doc["on"];
        if on.is_badvalue() {
            return vec!["no on: block".to_owned()];
        }
        let mut events = match on {
            Yaml::Hash(map) => map.keys().filter_map(scalar).collect(),
            other => all_texts(other),
        };
        events.sort_unstable();
        let mut problems = Vec::new();
        if events != ["push", "workflow_dispatch"] {
            problems.push(format!("triggers are {events:?}"));
        }
        let push = &on["push"];
        let filters = push
            .as_hash()
            .into_iter()
            .flat_map(|map| map.keys())
            .filter(|key| scalar(key).as_deref() != Some("tags"));
        problems.extend(filters.map(|key| format!("push is filtered by: {key:?}")));
        let tags = all_texts(&push["tags"]);
        if tags != ["v*"] {
            problems.push(format!("push tags are {tags:?}, not [\"v*\"]"));
        }
        problems
    })
}

// ---- run scripts ----

/// A `${{ }}` inside a `run:` string is substituted before the shell reads
/// it, so anything attacker-controlled in it (a branch name, an issue title)
/// runs. The scripts here need none: none is allowed (the whole string is
/// scanned, comments included), and `github.event` is used nowhere. A
/// workflow in which no script is found is a problem too.
fn injection_problems(text: &str) -> Vec<String> {
    on_tree(text, |doc| {
        let scripts = scripts(doc);
        let mut problems = Vec::new();
        if scripts.is_empty() {
            problems.push("no run scripts found".to_owned());
        }
        let expressions = scripts.iter().filter(|script| script.contains("${{"));
        problems.extend(expressions.map(|script| format!("expression in a run script: {script}")));
        let events = all_texts(doc)
            .into_iter()
            .filter(|value| value.contains("github.event"));
        problems.extend(events.map(|value| format!("github.event used: {value}")));
        problems
    })
}

/// A script without its shell comments (whole-line, and a trailing ` # ...`).
/// Used to decide whether a command is present, where losing text can make a
/// check fail but never pass, and where a step's first cargo command is
/// (`installer_check_problems`, the `Required` cache rule): there a `cargo` after a ` #`
/// inside quotes is lost, so that step does not count as a cargo step.
fn without_comments(script: &str) -> String {
    let code = script
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'));
    code.map(|line| line.split(" #").next().unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A step's `run` script without its shell comments; empty for a step that
/// runs no script.
fn step_code(step: &Yaml) -> String {
    scalar(&step["run"]).map_or_else(String::new, |script| without_comments(&script))
}

/// Whether a script (comments already removed) runs cargo: it has the word
/// `cargo`.
fn runs_cargo(code: &str) -> bool {
    code.split_whitespace().any(|word| word == "cargo")
}

/// Whether a script (comments already removed) runs `cargo test`: the words
/// `cargo` and `test` next to each other, on any line.
fn runs_cargo_test(code: &str) -> bool {
    code.lines().any(|line| {
        let words = line.split_whitespace().collect::<Vec<_>>();
        words.windows(2).any(|pair| pair == ["cargo", "test"])
    })
}

/// Whether a step is the `Swatinem/rust-cache` action, however its name is cased.
fn is_rust_cache(step: &Yaml) -> bool {
    uses_of(step).is_some_and(|uses| {
        uses.to_ascii_lowercase()
            .starts_with("swatinem/rust-cache@")
    })
}

/// No script found at all, or a command in `required` that no script contains
/// outside a shell comment.
fn missing_commands(text: &str, required: &[&str]) -> Vec<String> {
    on_tree(text, |doc| {
        let scripts = scripts(doc);
        let mut problems = Vec::new();
        if scripts.is_empty() {
            problems.push("no run scripts found".to_owned());
        }
        let code = scripts
            .iter()
            .map(|script| without_comments(script))
            .collect::<Vec<_>>()
            .join("\n");
        problems.extend(
            required
                .iter()
                .filter(|command| !code.contains(*command))
                .map(|command| format!("missing: {command}")),
        );
        problems
    })
}

// ---- structure and releases ----

/// The words of `text` that name a release: any word containing `release` (an
/// action, `gh release`, a `.../releases` call, `releaseId`) and `tauri-action`
/// (which makes releases). The only exceptions are the exact word `--release`
/// (a cargo or tauri flag) and the start `target/release/` of a word (the
/// build folder); the rest of that word is still read.
fn release_hits(text: &str) -> Vec<String> {
    let words = text
        .split_whitespace()
        .map(|word| word.trim_matches(|c| c == '\'' || c == '"'));
    let words = words.filter(|word| *word != "--release");
    let words = words.map(|word| word.strip_prefix("./").unwrap_or(word));
    let words = words.map(|word| word.strip_prefix("target/release/").unwrap_or(word));
    let release = |word: &&str| {
        ["release", "tauri-action"]
            .iter()
            .any(|bad| word.to_ascii_lowercase().contains(bad))
    };
    words.filter(release).map(str::to_owned).collect()
}

/// `defaults.run.shell` is `bash` (so the quoting in the scripts means the
/// same on every step); every `node-version` is a number of at least 22 (so
/// the quoted glob of `node --test` is expanded by node), and there is one.
/// What an upload sets is `upload_problems`' business.
fn setting_problems(text: &str) -> Vec<String> {
    on_tree(text, |doc| {
        let mut problems = Vec::new();
        let shell = scalar(&doc["defaults"]["run"]["shell"]);
        if shell.as_deref() != Some("bash") {
            problems.push(format!("defaults.run.shell is {shell:?}, not bash"));
        }
        let mut versions = 0;
        for step in all_steps(doc) {
            let version = &step["with"]["node-version"];
            if !version.is_badvalue() {
                versions += 1;
                let version = scalar(version).unwrap_or_default();
                let major: String = version.chars().take_while(char::is_ascii_digit).collect();
                if !major.parse::<u32>().is_ok_and(|major| major >= 22) {
                    problems.push(format!("node-version is not 22 or newer: {version}"));
                }
            }
        }
        problems.extend((versions == 0).then(|| "no node-version".to_owned()));
        problems
    })
}

/// When the platform has a `cache_save_if`: there is a `Swatinem/rust-cache`
/// step, and every one (in any job) has `with.save-if` equal to it, compared
/// as parsed text with the outer whitespace trimmed. `scalar` turns the parsed
/// value into text, so quoting (`'...'`, `"..."`) and a block scalar (which
/// ends in a newline) read the same. The inside of the expression is compared
/// as written: another spacing of the same expression, another ref, a
/// boolean, a list or a missing `with` are all refused, so a change to the
/// condition is a change to this rule, never a silent one.
fn cache_save_problems(text: &str, platform: &Platform) -> Vec<String> {
    on_tree(text, |doc| {
        // No requirement: the file is still parsed, so one that does not parse
        // is reported by every check.
        let Some(wanted) = platform.cache_save_if else {
            return Vec::new();
        };
        let mut problems = Vec::new();
        let mut caches = 0;
        for step in all_steps(doc)
            .into_iter()
            .filter(|step| is_rust_cache(step))
        {
            caches += 1;
            let save_if = &step["with"]["save-if"];
            match scalar(save_if) {
                Some(value) if value.trim() == wanted => {}
                Some(value) => problems.push(format!(
                    "a rust-cache step's save-if is {value:?}, not {wanted:?}"
                )),
                None if save_if.is_badvalue() => {
                    problems.push("a rust-cache step has no save-if".to_owned());
                }
                None => problems.push(format!(
                    "a rust-cache step's save-if is not a single value: {save_if:?}"
                )),
            }
        }
        problems.extend((caches == 0).then(|| "no rust-cache step to check".to_owned()));
        problems
    })
}

/// Whether a step has `if: success() || failure()`, exactly: the condition
/// that lets it run after an earlier step failed, but not after a cancel.
fn runs_after_failure(step: &Yaml) -> bool {
    scalar(&step["if"]).as_deref() == Some(AFTER_FAILURE)
}

/// There is a step that runs
/// `cargo test` (outside shell comments), and every such step has
/// `if: success() || failure()`, so it still runs after a failed formatting or
/// lint step and one run reports every failure (`node_step_problems` is the
/// same rule for the window script tests).
fn rust_test_problems(text: &str) -> Vec<String> {
    on_tree(text, |doc| {
        let steps = all_steps(doc)
            .into_iter()
            .filter(|step| runs_cargo_test(&step_code(step)))
            .collect::<Vec<_>>();
        let mut problems = steps
            .iter()
            .filter(|step| !runs_after_failure(step))
            .map(|step| {
                format!(
                    "a cargo test step without `if: {AFTER_FAILURE}`: {:?}",
                    step["if"]
                )
            })
            .collect::<Vec<_>>();
        problems.extend(steps.is_empty().then(|| "no cargo test step".to_owned()));
        problems
    })
}

/// The condition that lets a step run after an earlier step failed, but not
/// after the run was cancelled.
const AFTER_FAILURE: &str = "success() || failure()";

/// Every step that runs the window script tests (`node --test` outside a
/// shell comment) must have `if: success() || failure()`, so it still runs
/// when the Rust tests before it failed and one run reports both.
fn node_step_problems(text: &str) -> Vec<String> {
    on_tree(text, |doc| {
        all_steps(doc)
            .into_iter()
            .filter(|step| {
                scalar(&step["run"])
                    .is_some_and(|run| without_comments(&run).contains("node --test"))
            })
            .filter(|step| !runs_after_failure(step))
            .map(|step| {
                format!(
                    "a node test step without `if: {AFTER_FAILURE}`: {:?}",
                    step["if"]
                )
            })
            .collect()
    })
}

/// The job with this id, if the workflow has it.
fn job_named<'a>(doc: &'a Yaml, name: &str) -> Option<&'a Yaml> {
    jobs(doc)
        .into_iter()
        .find(|(job, _)| job == name)
        .map(|(_, job)| job)
}

/// The words of one shell line, quotes trimmed.
fn words_of(line: &str) -> Vec<&str> {
    line.split_whitespace()
        .map(|word| word.trim_matches(|c| c == '\'' || c == '"'))
        .collect()
}

/// The jobs are exactly `test` and the platform's Installer jobs, each with
/// the platform's runner; `test` exists; every Installer job exists, needs
/// `test`, uploads with `actions/upload-artifact@` (what it uploads is
/// `upload_problems`' business) and follows the platform's `BuildCache`
/// (`Forbidden`: no action with `cache` in its name; `Required`: a
/// `Swatinem/rust-cache@` step before its first cargo step); runs are queued
/// per workflow and ref; and no key or value anywhere (`uses` and `run`
/// included) names a release.
fn structure_problems(text: &str, platform: &Platform) -> Vec<String> {
    on_tree(text, |doc| {
        let all = jobs(doc);
        let mut problems = Vec::new();
        if all.is_empty() {
            problems.push("no jobs".to_owned());
        }
        for (name, job) in &all {
            let wanted = if name == "test" {
                Some(platform.test_runner)
            } else {
                platform
                    .installers
                    .iter()
                    .find(|installer| installer.job == name)
                    .map(|installer| installer.runner)
            };
            let Some(wanted) = wanted else {
                problems.push(format!("unexpected job {name}"));
                continue;
            };
            let runner = &job["runs-on"];
            match scalar(runner) {
                None if runner.is_badvalue() => problems.push(format!("job {name}: no runs-on")),
                None => problems.push(format!(
                    "job {name}: runs-on is not a single runner name: {runner:?}"
                )),
                Some(runner) if runner != wanted => {
                    problems.push(format!("job {name}: runner is not {wanted}: {runner}"));
                }
                Some(_) => {}
            }
        }
        if job_named(doc, "test").is_none() {
            problems.push("no test job".to_owned());
        }
        for installer in platform.installers {
            let name = installer.job;
            let Some(build) = job_named(doc, name) else {
                problems.push(format!("no {name} job"));
                continue;
            };
            if !all_texts(&build["needs"]).iter().any(|need| need == "test") {
                problems.push(format!("the {name} job does not have needs: test"));
            }
            if !steps_of(build).into_iter().any(uploads_artifact) {
                problems.push(format!("the {name} job does not upload an artifact"));
            }
            match platform.build_cache {
                BuildCache::Forbidden => {
                    let caches = steps_of(build)
                        .into_iter()
                        .filter_map(uses_of)
                        .filter(|uses| uses.to_ascii_lowercase().contains("cache"));
                    problems.extend(
                        caches.map(|uses| format!("the {name} job restores a cache: {uses}")),
                    );
                }
                BuildCache::Required => {
                    let steps = steps_of(build);
                    let cache = steps.iter().position(|step| is_rust_cache(step));
                    let cargo = steps.iter().position(|step| runs_cargo(&step_code(step)));
                    match (cache, cargo) {
                        (None, _) => problems.push(format!(
                            "the {name} job does not restore Swatinem/rust-cache"
                        )),
                        (Some(cache), Some(cargo)) if cache > cargo => problems.push(format!(
                            "the {name} job restores its cache after its first cargo step"
                        )),
                        (Some(_), _) => {}
                    }
                }
            }
        }
        let group = "${{ github.workflow }}-${{ github.ref }}";
        if doc["concurrency"].is_badvalue() {
            problems.push("no concurrency".to_owned());
        } else if scalar(&doc["concurrency"]["group"]).as_deref() != Some(group) {
            problems.push(format!("the concurrency group is not `{group}`"));
        }
        let hits = all_texts(doc)
            .iter()
            .flat_map(|value| release_hits(value))
            .collect::<Vec<_>>();
        problems.extend(hits.into_iter().map(|hit| format!("release step: {hit}")));
        problems
    })
}

/// The days an uploaded artifact is kept; the action's default is 90.
const RETENTION_DAYS: &str = "30";

/// Every Installer job has an upload step, and every upload (in any job)
/// belongs to an Installer job and matches that job's artifact:
/// - `name` is the job's artifact;
/// - `path` is a text whose trimmed, non-empty lines are exactly the job's
///   `artifact_paths`, as a set (`str::lines` and `trim`, so order, blank
///   lines, trailing spaces, tabs and Windows line endings do not matter, and a
///   `!` exclusion line or a folded single line does);
/// - `if-no-files-found` is `error` (the action's default only warns);
/// - `retention-days` is 30. `scalar` turns the parsed value into text, so
///   every spelling that YAML 1.2 reads as the integer 30 (`30`, `030`, `+30`,
///   `0x1E`, `0o36`) and the string `"30"` are accepted; anything else
///   (`30.0`, the string `"030"`, a list, null, any other number) is refused.
///
/// Any other step that sets `if-no-files-found` must set it to `error` too.
/// One that is set but is not a single value (a list, null, a mapping) is
/// reported as such, apart from an upload that does not set it at all.
fn upload_problems(text: &str, platform: &Platform) -> Vec<String> {
    on_tree(text, |doc| {
        let mut problems = Vec::new();
        for installer in platform.installers {
            let uploads = job_named(doc, installer.job).map(|job| {
                steps_of(job)
                    .into_iter()
                    .filter(|s| uploads_artifact(s))
                    .count()
            });
            if uploads == Some(0) {
                problems.push(format!("no upload-artifact step in job {}", installer.job));
            }
        }
        for (job_name, job) in jobs(doc) {
            let installer = platform
                .installers
                .iter()
                .find(|installer| installer.job == job_name);
            for step in steps_of(job) {
                let with = &step["with"];
                let found = &with["if-no-files-found"];
                let upload = uploads_artifact(step);
                if upload || !found.is_badvalue() {
                    match scalar(found).as_deref() {
                        Some("error") => {}
                        Some(value) => {
                            problems.push(format!("if-no-files-found is {value}, not error"));
                        }
                        None if found.is_badvalue() => {
                            problems.push("an upload does not set if-no-files-found".to_owned());
                        }
                        None => problems.push(format!(
                            "if-no-files-found is not a single value: {found:?}"
                        )),
                    }
                }
                if !upload {
                    continue;
                }
                let Some(installer) = installer else {
                    problems.push(format!(
                        "an upload in job {job_name}, which is not an installer job"
                    ));
                    continue;
                };
                let wanted: BTreeSet<&str> = installer.artifact_paths.iter().copied().collect();
                let (name, path, days) = (&with["name"], &with["path"], &with["retention-days"]);
                match scalar(name) {
                    None if name.is_badvalue() => {
                        problems.push("an upload does not set a name".to_owned());
                    }
                    None => {
                        problems.push(format!("an upload's name is not a single value: {name:?}"));
                    }
                    Some(name) if name != installer.artifact => problems.push(format!(
                        "an upload is named {name}, not {}",
                        installer.artifact
                    )),
                    Some(_) => {}
                }
                match scalar(path) {
                    None if path.is_badvalue() => {
                        problems.push("an upload does not set a path".to_owned());
                    }
                    None => problems.push(format!(
                        "an upload's path is not a block of lines: {path:?}"
                    )),
                    Some(path) => {
                        let lines: BTreeSet<&str> = path
                            .lines()
                            .map(str::trim)
                            .filter(|line| !line.is_empty())
                            .collect();
                        if lines != wanted {
                            problems
                                .push(format!("an upload's paths are {lines:?}, not {wanted:?}"));
                        }
                    }
                }
                match scalar(days) {
                    None if days.is_badvalue() => {
                        problems.push("an upload does not set retention-days".to_owned());
                    }
                    None => problems.push(format!(
                        "an upload's retention-days is not a number: {days:?}"
                    )),
                    Some(days) if days != RETENTION_DAYS => problems.push(format!(
                        "an upload's retention-days is {days}, not {RETENTION_DAYS}"
                    )),
                    Some(_) => {}
                }
            }
        }
        problems
    })
}

// ---- the bundle build ----

/// The config file that adds the ffmpeg and ffprobe sidecars and stages them
/// first (`beforeBuildCommand`); only the bundle build reads it.
const BUNDLE_CONFIG: &str = "src-tauri/tauri.bundle.conf.json";

/// Whether the words of one command line pass `--config BUNDLE_CONFIG` (or
/// `--config=BUNDLE_CONFIG`).
fn passes_bundle_config(words: &[&str]) -> bool {
    let joined = format!("--config={BUNDLE_CONFIG}");
    words.contains(&joined.as_str())
        || words
            .windows(2)
            .any(|pair| pair == ["--config", BUNDLE_CONFIG])
}

/// Every Installer job runs `cargo tauri build`, and every such line in it,
/// outside shell comments, passes the bundle config. Without it the installer
/// ships without ffmpeg and ffprobe, and the app cannot read or encode a
/// video.
fn bundle_build_problems(text: &str, platform: &Platform) -> Vec<String> {
    on_tree(text, |doc| {
        let mut problems = Vec::new();
        for installer in platform.installers {
            let name = installer.job;
            let Some(build) = job_named(doc, name) else {
                problems.push(format!("no {name} job"));
                continue;
            };
            let scripts = steps_of(build)
                .into_iter()
                .filter_map(|step| scalar(&step["run"]))
                .map(|script| without_comments(&script))
                .collect::<Vec<_>>();
            let builds = scripts
                .iter()
                .flat_map(|script| script.lines())
                .map(words_of)
                .filter(|words| words.windows(3).any(|w| w == ["cargo", "tauri", "build"]))
                .collect::<Vec<_>>();
            if builds.is_empty() {
                problems.push(format!("the {name} job has no cargo tauri build"));
                continue;
            }
            let plain = builds.iter().filter(|words| !passes_bundle_config(words));
            problems.extend(plain.map(|words| {
                format!(
                    "a bundle build without --config {BUNDLE_CONFIG} in job {name}: {}",
                    words.join(" ")
                )
            }));
        }
        problems
    })
}

// ---- ffmpeg for the tests ----

/// The variable the core and the CLI read to find ffmpeg and ffprobe.
const TOOLS_DIR_VAR: &str = "VIDEO_OPTIMIZER_TOOLS_DIR";

/// Where the test job stages ffmpeg and ffprobe.
const STAGE_DIR: &str = "$RUNNER_TEMP/ffmpeg";

/// The words of the staging command, before the folder.
const STAGE_WORDS: [&str; 8] = [
    "cargo",
    "run",
    "--locked",
    "-p",
    "video-optimizer-xtask",
    "--",
    "ffmpeg",
    "--dest",
];

/// The test job stages ffmpeg and ffprobe with
/// `cargo run --locked -p video-optimizer-xtask -- ffmpeg --dest "$RUNNER_TEMP/ffmpeg"`
/// and exports `VIDEO_OPTIMIZER_TOOLS_DIR` with that folder through
/// `$GITHUB_ENV`, each in a step before the first `cargo test` (an
/// environment file only reaches the steps after the one that wrote it). Both
/// are read from the scripts without their shell comments, so a command that
/// is only commented does not count.
fn tools_dir_problems(text: &str) -> Vec<String> {
    on_tree(text, |doc| {
        let Some(test) = job_named(doc, "test") else {
            return vec!["no test job".to_owned()];
        };
        let code = steps_of(test)
            .into_iter()
            .map(step_code)
            .collect::<Vec<_>>();
        let on_a_line = |wanted: &dyn Fn(&[&str]) -> bool| {
            code.iter()
                .position(|script| script.lines().any(|line| wanted(&words_of(line))))
        };
        let stage = on_a_line(&|words| {
            words
                .windows(STAGE_WORDS.len() + 1)
                .any(|w| w[..STAGE_WORDS.len()] == STAGE_WORDS && w[STAGE_WORDS.len()] == STAGE_DIR)
        });
        let export = format!("{TOOLS_DIR_VAR}={STAGE_DIR}");
        let env = on_a_line(&|words| {
            words.contains(&export.as_str()) && words.windows(2).any(|w| w == [">>", "$GITHUB_ENV"])
        });
        let tests = code.iter().position(|script| runs_cargo_test(script));
        let mut problems = Vec::new();
        if stage.is_none() {
            problems.push(format!(
                "the test job does not stage ffmpeg with --dest \"{STAGE_DIR}\""
            ));
        }
        if env.is_none() {
            problems.push(format!(
                "the test job does not set {TOOLS_DIR_VAR} to {STAGE_DIR} through $GITHUB_ENV"
            ));
        }
        match tests {
            None => problems.push("no cargo test step in the test job".to_owned()),
            Some(tests) => {
                if stage.is_some_and(|stage| stage >= tests) {
                    problems
                        .push("the test job stages ffmpeg after its first cargo test".to_owned());
                }
                if env.is_some_and(|env| env >= tests) {
                    problems.push(format!(
                        "the test job sets {TOOLS_DIR_VAR} after its first cargo test"
                    ));
                }
            }
        }
        problems
    })
}

// ---- the installer checks ----

/// What the Windows check step must test, one `test -f` line each: the app
/// and both programs beside it in the build folder (the NSIS installer takes
/// them from there).
/// The script every Mac Installer job runs on its built app and, inside the
/// mounted disk image, on the app people download; what it refuses is tested in
/// `check_mac_app.rs`. The job's whole check step is this one call.
const CHECK_SCRIPT: &str = "scripts/check-mac-app.sh";

/// The words of the call: the script with the job's architecture, which the
/// step takes from its own `env: EXPECTED_ARCH`.
const CHECK_CALL: [&str; 3] = ["bash", CHECK_SCRIPT, "$EXPECTED_ARCH"];

const WINDOWS_CHECK_LINES: [&str; 3] = [
    "test -f target/release/video-optimizer-app.exe",
    "test -f target/release/ffmpeg.exe",
    "test -f target/release/ffprobe.exe",
];

/// Every Installer job has a check step between the bundle build and the
/// upload, so a broken app or installer fails the run before anyone downloads
/// it. A Mac job (`arch` set): the first step of the job whose script, outside
/// shell comments, names `CHECK_SCRIPT`. That step is exactly the one call
/// `CHECK_CALL` (no `|| true`, no second command, no other script or argument:
/// a call that can pass when the script fails proves nothing), has no `if` and
/// no `continue-on-error`, and has `env: EXPECTED_ARCH` equal to the job's
/// `arch`. A Windows job: the first step that has the `WINDOWS_CHECK_LINES`
/// among its lines. Either comes after a step that runs `cargo tauri build` and
/// before the first upload. Like the other script checks this is a lint on the
/// `run:` text: it proves the check is called and in order, not what the
/// script does (`check_mac_app.rs` proves that).
fn installer_check_problems(text: &str, platform: &Platform) -> Vec<String> {
    on_tree(text, |doc| {
        let mut problems = Vec::new();
        for installer in platform.installers {
            let name = installer.job;
            let Some(build) = job_named(doc, name) else {
                problems.push(format!("no {name} job"));
                continue;
            };
            let steps = steps_of(build);
            let code = steps.iter().map(|step| step_code(step)).collect::<Vec<_>>();
            let lines = |script: &String| {
                script
                    .lines()
                    .map(|line| words_of(line).join(" "))
                    .collect::<Vec<_>>()
            };
            let check = if let Some(arch) = installer.arch {
                let Some(check) = code.iter().position(|script| script.contains(CHECK_SCRIPT))
                else {
                    problems.push(format!(
                        "job {name}: no check step: no step runs {CHECK_SCRIPT}"
                    ));
                    continue;
                };
                let calls = code[check]
                    .lines()
                    .map(words_of)
                    .filter(|words| !words.is_empty())
                    .collect::<Vec<_>>();
                if calls != [CHECK_CALL] {
                    problems.push(format!(
                        "job {name}: the check step must run exactly `{}`, not: {}",
                        CHECK_CALL.join(" "),
                        code[check].trim()
                    ));
                }
                for key in ["if", "continue-on-error"] {
                    if !steps[check][key].is_badvalue() {
                        problems.push(format!("job {name}: the check step has {key}"));
                    }
                }
                let expected = scalar(&steps[check]["env"]["EXPECTED_ARCH"]);
                if expected.as_deref() != Some(arch) {
                    problems.push(format!(
                        "job {name}: the check step's EXPECTED_ARCH is {expected:?}, not {arch}"
                    ));
                }
                check
            } else {
                let Some(check) = code.iter().position(|script| {
                    let have = lines(script);
                    WINDOWS_CHECK_LINES
                        .iter()
                        .any(|wanted| have.iter().any(|l| l == wanted))
                }) else {
                    problems.push(format!(
                        "job {name}: no check step: no step tests the files beside the app"
                    ));
                    continue;
                };
                let have = lines(&code[check]);
                for wanted in WINDOWS_CHECK_LINES {
                    if !have.iter().any(|line| line == wanted) {
                        problems.push(format!("job {name}: the check step does not run {wanted}"));
                    }
                }
                check
            };
            let bundles = |script: &String| {
                script.lines().any(|line| {
                    words_of(line)
                        .windows(3)
                        .any(|w| w == ["cargo", "tauri", "build"])
                })
            };
            if !code[..check].iter().any(bundles) {
                problems.push(format!(
                    "job {name}: the check step does not come after a cargo tauri build"
                ));
            }
            let upload = steps.iter().position(|step| uploads_artifact(step));
            if !upload.is_some_and(|upload| upload > check) {
                problems.push(format!(
                    "job {name}: the check step does not come before the upload"
                ));
            }
        }
        problems
    })
}

const REQUIRED_COMMANDS: &[&str] = &[
    "cargo fmt --all --check",
    "cargo clippy --workspace --all-targets -- -D warnings",
    "cargo test --workspace --no-fail-fast",
    "node --test \"tests/ui/*.test.js\"",
    "cargo install tauri-cli --locked --version 2.12.4",
    "cargo tauri build",
];

/// Every check, named, on `text` with `platform`'s profile.
fn every_check(text: &str, platform: &Platform) -> Vec<(&'static str, Vec<String>)> {
    vec![
        ("unpinned_actions", unpinned_actions(text)),
        ("permission_problems", permission_problems(text)),
        ("secret_references", secret_references(text)),
        ("trigger_problems", trigger_problems(text)),
        ("injection_problems", injection_problems(text)),
        (
            "missing_commands",
            missing_commands(text, REQUIRED_COMMANDS),
        ),
        ("structure_problems", structure_problems(text, platform)),
        ("setting_problems", setting_problems(text)),
        ("upload_problems", upload_problems(text, platform)),
        (
            "bundle_build_problems",
            bundle_build_problems(text, platform),
        ),
        (
            "installer_check_problems",
            installer_check_problems(text, platform),
        ),
        ("tools_dir_problems", tools_dir_problems(text)),
        ("node_step_problems", node_step_problems(text)),
        ("cache_save_problems", cache_save_problems(text, platform)),
        ("rust_test_problems", rust_test_problems(text)),
    ]
}

// ---- fixtures ----

const PIN: &str = "11bd71901bbe5b1630ceea73d27597364c9af683";

const BUILD_STEP: &str =
    "      - run: cargo tauri build --config src-tauri/tauri.bundle.conf.json\n";

const TAURI_INSTALL: &str = "      - run: cargo install tauri-cli --locked --version 2.12.4\n";

/// The test job's staging of ffmpeg, as `windows.yml` and `macos.yml` run it.
const STAGE_STEP: &str = r#"      - run: |
          cargo run --locked -p video-optimizer-xtask -- ffmpeg --dest "$RUNNER_TEMP/ffmpeg"
          echo "VIDEO_OPTIMIZER_TOOLS_DIR=$RUNNER_TEMP/ffmpeg" >> "$GITHUB_ENV"
"#;

const FMT_STEP: &str = "      - run: cargo fmt --all --check\n";

const RUST_TESTS: &str =
    "      - run: cargo test --workspace --no-fail-fast\n        if: success() || failure()\n";

const NODE_STEP: &str =
    "      - run: node --test \"tests/ui/*.test.js\"\n        if: success() || failure()\n";

/// The Windows Installer job's check step.
const WINDOWS_CHECK_STEP: &str = r#"      - run: |
          ls -l target/release/
          test -f target/release/video-optimizer-app.exe
          test -f target/release/ffmpeg.exe
          test -f target/release/ffprobe.exe
"#;

const UPLOAD_STEP: &str = "      - uses: actions/upload-artifact@PIN # v7
        with:
          name: video-optimizer-windows-installer
          path: target/release/bundle/nsis/*.exe
          if-no-files-found: error
          retention-days: 30
";

/// The head of both fixtures: triggers, permissions, queueing and the shell.
const HEAD: &str = "name: x
on:
  workflow_dispatch:
  push:
    tags:
      - 'v*'
permissions:
  contents: read
concurrency:
  group: ${{ github.workflow }}-${{ github.ref }}
defaults:
  run:
    shell: bash
jobs:
";

/// A Windows workflow every check accepts under `WINDOWS` (the Mac one is
/// `good_mac()`). It holds a cache in the `test` job, which is allowed there.
/// The variants below change one thing.
fn good() -> String {
    let test = format!(
        "  test:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@PIN # v7.0.1
      - uses: actions/setup-node@PIN # v7.1.0
        with:
          node-version: 22
      - uses: Swatinem/rust-cache@PIN # v2
{STAGE_STEP}{FMT_STEP}      - run: cargo clippy --workspace --all-targets -- -D warnings
        if: success() || failure()
{RUST_TESTS}{NODE_STEP}"
    );
    let build = format!(
        "  build:
    needs: test
    runs-on: windows-latest
    steps:
{TAURI_INSTALL}{BUILD_STEP}{WINDOWS_CHECK_STEP}{UPLOAD_STEP}"
    );
    format!("{HEAD}{test}{build}").replace("PIN", PIN)
}

/// The Mac test job's cache step: it saves only from `main`.
const MAC_SAVE_IF: &str =
    "        with:\n          save-if: ${{ github.ref == 'refs/heads/main' }}\n";

/// An Installer job's own cache in `good_mac()`.
const MAC_BUILD_CACHE: &str = "      - uses: Swatinem/rust-cache@PIN # v2.9.3\n        with:\n          save-if: ${{ github.ref == 'refs/heads/main' }}\n";

/// The check step of a Mac Installer job, as `macos.yml` has it: the one
/// call of the script (`scripts/check-mac-app.sh`, tested in
/// `check_mac_app.rs`) with the job's architecture. `@ARCH@` is that
/// architecture.
const MAC_CHECK_STEP: &str = r#"      - env:
          EXPECTED_ARCH: @ARCH@
        run: bash scripts/check-mac-app.sh "$EXPECTED_ARCH"
"#;

const MAC_UPLOAD_STEP: &str = "      - uses: actions/upload-artifact@PIN # v7
        with:
          name: @ARTIFACT@
          path: target/release/bundle/dmg/*.dmg
          if-no-files-found: error
          retention-days: 30
";

/// One Mac Installer job of `good_mac()`.
fn mac_installer_job(job: &str, runner: &str, arch: &str, artifact: &str) -> String {
    let check = MAC_CHECK_STEP.replace("@ARCH@", arch);
    let upload = MAC_UPLOAD_STEP.replace("@ARTIFACT@", artifact);
    format!(
        "  {job}:\n    needs: test\n    runs-on: {runner}\n    steps:\n{MAC_BUILD_CACHE}{TAURI_INSTALL}{BUILD_STEP}{check}{upload}"
    )
}

/// A Mac workflow every check accepts under `MACOS`: a cache in the test job
/// and in both Installer jobs that saves only from `main`, the check step
/// between each build and its upload, and no Windows path step.
fn good_mac() -> String {
    let test = format!(
        "  test:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@PIN # v7.0.1
      - uses: actions/setup-node@PIN # v7.1.0
        with:
          node-version: 22
      - uses: Swatinem/rust-cache@PIN # v2
{MAC_SAVE_IF}{STAGE_STEP}{FMT_STEP}      - run: cargo clippy --workspace --all-targets -- -D warnings
        if: success() || failure()
{RUST_TESTS}{NODE_STEP}"
    );
    let arm = mac_installer_job(
        "build",
        "macos-latest",
        "arm64",
        "video-optimizer-macos-arm64-dmg",
    );
    let intel = mac_installer_job(
        "build-intel",
        "macos-15-intel",
        "x86_64",
        "video-optimizer-macos-intel-dmg",
    );
    format!("{HEAD}{test}{arm}{intel}").replace("PIN", PIN)
}

/// `text` with the first `from` replaced by `to`. Fails when `from` is not
/// there, so a fixture cannot silently be a no-op.
fn replaced(text: &str, from: &str, to: &str) -> String {
    let from = from.replace("PIN", PIN);
    assert!(text.contains(&from), "the fixture lacks {from:?}");
    text.replacen(&from, &to.replace("PIN", PIN), 1)
}

/// `good()` with the first `from` replaced by `to`.
fn with(from: &str, to: &str) -> String {
    replaced(&good(), from, to)
}

/// `good_mac()` with the first `from` replaced by `to`.
fn with_mac(from: &str, to: &str) -> String {
    replaced(&good_mac(), from, to)
}

/// `text` with the first `from` inside the job `job` replaced by `to`. A job
/// runs from its `  <name>:` line to the next job's.
fn in_job(text: &str, job: &str, from: &str, to: &str) -> String {
    let head = format!("\n  {job}:\n");
    let start = text.find(&head).unwrap_or_else(|| panic!("no job {job}")) + 1;
    let after = start + head.len() - 1;
    // The next job's line: two spaces and a name, where the steps have more.
    let end = text[after..]
        .match_indices("\n  ")
        .map(|(at, _)| after + at + 1)
        .find(|at| text[at + 2..].chars().next().is_some_and(|c| c != ' '))
        .unwrap_or(text.len());
    let section = replaced(&text[start..end], from, to);
    format!("{}{}{}", &text[..start], section, &text[end..])
}

/// `good()` with one more step at the end of the build job (six-space indent).
fn add_step(step: &str) -> String {
    format!("{}{}\n", good(), step.replace("PIN", PIN))
}

/// A one-job workflow around the given steps.
fn steps_doc(steps: &str) -> String {
    let head = "jobs:\n  a:\n    runs-on: windows-latest\n    steps:\n";
    format!("{head}{}\n", steps.replace("PIN", PIN))
}

/// Both fixtures with the profile they are checked under.
fn both() -> [(String, &'static Platform); 2] {
    [(good(), &WINDOWS), (good_mac(), &MACOS)]
}

fn assert_clean(problems: Vec<String>) {
    assert_eq!(problems, Vec::<String>::new());
}

/// A problem that starts with `start` is among `problems`.
fn assert_problem(problems: &[String], start: &str, what: &str) {
    let found = problems.iter().any(|problem| problem.starts_with(start));
    assert!(
        found,
        "{what}: no problem starting {start:?} in {problems:?}"
    );
}

/// `check` reports a problem that starts with `start` on every text.
fn assert_all_caught(check: impl Fn(&str) -> Vec<String>, start: &str, texts: &[String]) {
    texts
        .iter()
        .for_each(|text| assert_problem(&check(text), start, text));
}

// ---- the real files ----

#[test]
fn both_workflows_parse_as_one_yaml_mapping_and_on_is_a_name() {
    for platform in [&WINDOWS, &MACOS] {
        let doc = parse(&workflow(platform)).unwrap_or_else(|e| panic!("{e}"));
        let hash = doc.as_hash().expect("a mapping");
        // YAML 1.2: `on` is the name `on`. A YAML 1.1 reader makes it boolean true.
        assert!(hash.contains_key(&Yaml::String("on".to_owned())));
        assert!(!hash.keys().any(|key| matches!(key, Yaml::Boolean(_))));
        assert!(doc["on"].as_hash().is_some());
    }
}

#[test]
fn both_workflows_pass_every_check() {
    for platform in [&WINDOWS, &MACOS] {
        for (name, problems) in every_check(&workflow(platform), platform) {
            assert_eq!(problems, Vec::<String>::new(), "{}: {name}", platform.file);
        }
    }
}

#[test]
fn the_checks_of_the_real_files_are_not_vacuous() {
    for (platform, refs, scripts, caches) in [(&WINDOWS, 7, 10, 1), (&MACOS, 12, 11, 3)] {
        let text = workflow(platform);
        let (found_refs, found_scripts) =
            (action_references(&text).len(), run_scripts(&text).len());
        assert!(
            found_refs >= refs && found_scripts >= scripts,
            "{}: {found_refs} action references, {found_scripts} run scripts",
            platform.file
        );
        assert!(text.contains("target/release/bundle/"));
        let doc = parse(&text).unwrap_or_else(|e| panic!("{e}"));
        let found_caches = all_steps(&doc)
            .into_iter()
            .filter(|step| is_rust_cache(step))
            .count();
        assert_eq!(found_caches, caches, "{}: rust-cache steps", platform.file);
        let tests = all_steps(&doc)
            .into_iter()
            .filter(|step| runs_cargo_test(&step_code(step)))
            .count();
        assert_eq!(tests, 1, "{}: one cargo test step", platform.file);
        // One job for the tests and the profile's installer jobs, no more.
        assert_eq!(jobs(&doc).len(), 1 + platform.installers.len());
    }
}

// ---- parsing ----

#[test]
fn a_well_formed_workflow_passes_every_check_in_a_fixture() {
    for (text, platform) in both() {
        for (name, problems) in every_check(&text, platform) {
            assert_eq!(problems, Vec::<String>::new(), "{}: {name}", platform.file);
        }
    }
}

#[test]
#[should_panic(expected = "the fixture lacks")]
fn a_fixture_change_that_finds_nothing_fails() {
    with("a step this fixture does not have", "");
}

#[test]
fn a_workflow_that_does_not_parse_is_reported_by_every_check_in_a_fixture() {
    for bad in [
        "jobs: [unclosed\n",
        "",
        "a: 1\n---\nb: 2\n",
        "- a\n- b\n",
        "a: 1\na: 2\n",
    ] {
        for platform in [&WINDOWS, &MACOS] {
            for (name, problems) in every_check(bad, platform) {
                assert_problem(&problems, "the workflow", &format!("{name} on {bad:?}"));
            }
        }
    }
}

#[test]
fn a_true_key_is_not_read_as_on_in_a_fixture() {
    // The parser reads `on` as a name, and a real boolean key is not `on`.
    assert_eq!(parse("on: x\n").unwrap()["on"].as_str(), Some("x"));
    assert_eq!(
        trigger_problems("true:\n  workflow_dispatch:\n"),
        ["no on: block"]
    );
}

// ---- pinned actions ----

#[test]
#[rustfmt::skip]
fn an_action_pinned_to_a_tag_or_branch_is_caught_in_a_fixture() {
    let upper = PIN.to_uppercase();
    let bad = [
        "uses: actions/checkout@v4", "uses: actions/checkout@main", "uses: actions/checkout", "uses: actions/checkout@11bd719",
        "uses: ./local-action", "uses: docker://alpine:3.20",
        // 41 characters, and 40 with an upper-case digit.
        &format!("uses: actions/checkout@{PIN}0 # v4"), &format!("uses: actions/checkout@{upper} # v4"),
        // The same mistake in spellings a line scan would miss.
        " uses:   actions/checkout@v4", "uses: 'actions/checkout@v4'", "uses: \"actions/checkout@v4\"",
        "{ uses: actions/checkout@v4 }", "name: x\n        uses: actions/checkout@v4",
    ];
    for step in bad {
        let problems = unpinned_actions(&steps_doc(&format!("      - {step}")));
        assert_eq!(problems.len(), 1, "{step}: {problems:?}");
        assert_problem(&problems, "not pinned", step);
    }
    // A reusable workflow called by a job is an action reference too.
    assert_problem(&unpinned_actions("jobs:\n  a:\n    uses: org/repo/.github/workflows/x.yml@main\n"), "not pinned", "job");
}

#[test]
fn a_pinned_action_without_a_release_comment_is_caught_in_a_fixture() {
    for step in [
        format!("      - uses: actions/checkout@{PIN}"),
        format!("      - uses: actions/checkout@{PIN} #"),
    ] {
        assert_problem(
            &unpinned_actions(&steps_doc(&step)),
            "no release comment",
            &step,
        );
    }
    let job = format!("jobs:\n  a:\n    uses: org/repo/.github/workflows/x.yml@{PIN}\n");
    assert_problem(
        &unpinned_actions(&job),
        "no release comment",
        "job-level uses",
    );
}

#[test]
fn a_pin_used_twice_needs_a_comment_on_every_line_in_a_fixture() {
    let line = format!("      - uses: actions/checkout@{PIN}");
    assert_eq!(
        unpinned_actions(&steps_doc(&format!("{line} # v4\n{line}"))).len(),
        1
    );
    assert_clean(unpinned_actions(&steps_doc(&format!(
        "{line} # v4\n{line} # v4"
    ))));
}

#[test]
fn an_action_pinned_to_a_commit_with_a_release_comment_passes_a_fixture() {
    let pin = format!("actions/checkout@{PIN}");
    for ok in [
        format!("      - uses: {pin} # v4.2.2"),
        format!("      -   uses:   {pin}   # v4.2.2"),
        format!("      - uses: '{pin}' # v4.2.2"),
        format!("      - name: x\n        uses: {pin} # v4.2.2"),
        // `uses` inside `with` is an input of an action, not an action.
        "      - name: x\n        run: ls\n        with: { uses: a/b@v1 }".to_owned(),
    ] {
        assert_clean(unpinned_actions(&steps_doc(&ok)));
    }
    assert_clean(unpinned_actions(&format!(
        "jobs:\n  a:\n    uses: o/r/.github/workflows/x.yml@{PIN} # v1\n"
    )));
    // A comment line that mentions `uses:` is not a reference.
    assert_clean(unpinned_actions(&format!(
        "# uses: {pin}@v4 is bad\n{}",
        steps_doc("      - run: ls")
    )));
}

#[test]
#[rustfmt::skip]
fn action_references_are_read_from_the_tree_in_a_fixture() {
    let steps = "      - uses: a/b@v1\n      -  uses: c/d@v1\n      - {uses: e/f@v1}\n      - uses: 'g/h@v1'\n      - name: x\n        with: { uses: i/j@v1 }";
    let text = format!("{}# uses: k/l@v1\n", steps_doc(steps));
    assert_eq!(action_references(&text), ["a/b@v1", "c/d@v1", "e/f@v1", "g/h@v1"]);
}

// ---- permissions ----

#[test]
#[rustfmt::skip]
fn a_write_permission_is_caught_in_a_fixture() {
    let top = |block: &str| permission_problems(&format!("permissions:{block}\njobs:\n  a: {{}}\n"));
    assert_problem(&top("\n  contents: write"), "write permission", "write");
    assert_problem(&top("\n  contents: write"), "top-level permissions", "write");
    assert_eq!(top("\n  contents: read\n  id-token: write").len(), 2);
    assert_problem(&top(" write-all"), "write permission", "write-all");
    assert_eq!(top("\n  contents: read\n  packages: read").len(), 1, "wider than read");
    assert_eq!(permission_problems("name: x\njobs:\n  a: {}\n"), ["no top-level permissions block"]);
    // Read-only at the top, but a job asks for more.
    let job = |block: &str| permission_problems(&format!("permissions:\n  contents: read\njobs:\n  a:\n    permissions:{block}\n"));
    assert_eq!(job("\n      contents: write").len(), 1);
    assert_eq!(job(" write-all").len(), 1);
    // The same, in each real fixture.
    for (text, _) in both() {
        assert_problem(&permission_problems(&replaced(&text, "contents: read", "contents: write")), "write permission", "fixture");
    }
}

#[test]
#[rustfmt::skip]
fn a_quoted_write_permission_is_caught_in_a_fixture() {
    let job = "permissions:\n  contents: read\njobs:\n  a:\n    permissions:";
    for value in ["'write'", "\"write\"", "  write", "'write-all'", "Write", "{ contents: 'write' }"] {
        let top = format!("permissions:\n  contents: {value}\njobs:\n  a: {{}}\n");
        let in_job = format!("{job}\n      contents: {value}\n");
        let flow = format!("{job} {{contents: {value}}}\n");
        for text in [top, in_job, flow] {
            assert_problem(&permission_problems(&text), "write permission", &text);
        }
    }
}

#[test]
fn read_only_contents_permission_passes_a_fixture() {
    for read in [
        "read",
        "'read'",
        "\"read\"",
        "  read",
        "read # nothing else",
    ] {
        assert_clean(permission_problems(&format!(
            "name: x\n\npermissions:\n  contents: {read}\n\njobs:\n  a: {{}}\n"
        )));
    }
    assert_clean(permission_problems(
        "permissions:\n  contents: read\njobs:\n  a:\n    permissions:\n      contents: 'read'\n",
    ));
}

// ---- secrets and tokens ----

#[test]
#[rustfmt::skip]
fn a_secret_or_token_reference_is_caught_in_a_fixture() {
    for value in ["${{ secrets.GITHUB_TOKEN }}", "${{secrets.NPM}}", "${{ secrets['NPM'] }}", "${{ toJSON(secrets) }}", "${{ SECRETS.NPM }}"] {
        let step = format!("      - run: ls\n        env:\n          P: {value}");
        assert_eq!(secret_references(&steps_doc(&step)).len(), 1, "{value}");
    }
    let inherit = "jobs:\n  a:\n    uses: o/r/.github/workflows/x.yml@main\n    secrets: inherit\n";
    assert_eq!(secret_references(inherit).len(), 1);
    for bad in [
        "      - run: ls\n        with:\n          token: ${{ github.token }}",
        "      - run: ls\n        env:\n          GITHUB_TOKEN: abc",
        "      - run: echo $GITHUB_TOKEN",
        "      - uses: a/b@PIN # v1\n        with:\n          github-token: x",
    ] {
        assert_eq!(secret_references(&steps_doc(bad)).len(), 1, "{bad}");
    }
    // In each real fixture.
    for (text, _) in both() {
        let text = replaced(&text, FMT_STEP, &format!("{FMT_STEP}        env:\n          T: ${{{{ secrets.X }}}}\n"));
        assert_eq!(secret_references(&text).len(), 1);
    }
}

#[test]
#[rustfmt::skip]
fn a_workflow_without_secret_words_passes_a_fixture() {
    let steps = "      - run: ls # secrets.X\n      - name: mysecretsauce\n        run: echo secrets_manager\n        env:\n          NAME: github-actions";
    assert_clean(secret_references(&format!("# no secrets.TOKEN here\n{}", steps_doc(steps))));
    for (text, _) in both() {
        assert_clean(secret_references(&text));
    }
}

// ---- triggers ----

#[test]
#[rustfmt::skip]
fn a_pull_request_trigger_or_a_branch_push_is_caught_in_a_fixture() {
    let tags = "    tags:\n      - 'v*'\n";
    let events = |on: &str| trigger_problems(&format!("on:\n{on}"));
    assert_problem(&events("  pull_request_target:\n    types: [opened]\n"), "triggers are", "target");
    let pr = events(&format!("  workflow_dispatch:\n  pull_request:\n  push:\n{tags}"));
    assert_eq!(pr.len(), 1);
    assert_problem(&pr, "triggers are", "pull_request");
    for filter in ["branches:\n      - main", "tags-ignore:\n      - 'x*'", "paths:\n      - 'a/**'"] {
        let problems = events(&format!("  workflow_dispatch:\n  push:\n    {filter}\n{tags}"));
        assert_eq!(problems.len(), 1, "{filter}");
        assert_problem(&problems, "push is filtered by", filter);
    }
    assert_problem(&events("  workflow_dispatch:\n  push:\n    tags: ['v*', '*']\n"), "push tags are", "extra tag");
    assert_problem(&events("  workflow_dispatch:\n  push:\n"), "push tags are", "no filter");
    assert_problem(&events(&format!("  push:\n{tags}")), "triggers are", "push only");
    assert_eq!(trigger_problems("jobs:\n  a: {}\n"), ["no on: block"]);
    assert_problem(&trigger_problems("on: pull_request\n"), "triggers are", "scalar");
    assert_problem(&trigger_problems("on: [push, pull_request]\n"), "triggers are", "list");
    // In each real fixture.
    for (text, _) in both() {
        let text = replaced(&text, "  workflow_dispatch:\n", "  workflow_dispatch:\n  pull_request:\n");
        assert_problem(&trigger_problems(&text), "triggers are", "fixture");
    }
}

#[test]
#[rustfmt::skip]
fn manual_start_and_a_version_tag_pass_a_fixture() {
    for ok in [
        "on:\n  workflow_dispatch:\n  push:\n    tags:\n      - 'v*'\n",
        "on:\n  push:\n    tags:\n      - \"v*\"\n  workflow_dispatch:\n",
        "on:\n  workflow_dispatch:\n  push:\n    tags: ['v*']\n",
        "on: { push: { tags: ['v*'] }, workflow_dispatch: {} }\n",
    ] {
        assert_clean(trigger_problems(ok));
    }
}

// ---- run scripts ----

#[test]
fn block_scripts_of_every_style_are_read_in_a_fixture() {
    let steps = "      - run: echo a
      - run: |-
          b

          c
      - run: >
          d
          e
      - run: >-
          f
      - run: |+
          g
      - run: |
          h
        shell: bash
      - name: n
        run: |
          i
        env:
          X: y
      -  run: \"quoted\"
      - run: 'single'";
    let expected = [
        "echo a", "b\n\nc", "d e\n", "f", "g\n", "h\n", "i\n", "quoted", "single",
    ];
    assert_eq!(run_scripts(&steps_doc(steps)), expected);
}

#[test]
fn a_hash_after_a_space_in_a_plain_run_is_a_yaml_comment_in_a_fixture() {
    // YAML ends a plain value at ` #`, so the runner never sees the rest, and
    // neither does the parser: what it reads is what would run. The same text
    // in quotes or in a block is read whole (see the release fixtures).
    let plain = steps_doc("      - run: echo \"a #b\"; gh release create v1");
    assert_eq!(run_scripts(&plain), ["echo \"a"]);
    let quoted = steps_doc("      - run: 'echo \"a #b\"; gh release create v1'");
    assert_eq!(
        run_scripts(&quoted),
        ["echo \"a #b\"; gh release create v1"]
    );
}

#[test]
#[rustfmt::skip]
fn an_expression_in_a_script_is_caught_in_a_fixture() {
    let inline = injection_problems(&steps_doc("      - run: echo ${{ github.event.pull_request.title }}"));
    assert_problem(&inline, "expression in a run script", "inline");
    assert_problem(&inline, "github.event used", "inline");
    for step in [
        "      - name: x\n        run: |\n          echo start\n          echo \"${{ github.ref_name }}\"\n      - run: ls",
        // The whole string is scanned, a shell comment inside it included.
        "      - run: |\n          ls\n          # ${{ github.ref_name }}",
        "      - run: \"echo ${{ github.ref_name }}\"",
    ] {
        assert_eq!(injection_problems(&steps_doc(step)).len(), 1, "{step}");
    }
    // Not a run script, so only the github.event rule applies.
    let elsewhere = "      - run: ls\n      - uses: a/b@x\n        with:\n          name: ${{ github.event.head_commit.message }}";
    let problems = injection_problems(&steps_doc(elsewhere));
    assert_eq!(problems.len(), 1);
    assert_problem(&problems, "github.event used", "with");
    // In each real fixture, and a fixture's own `${{ }}` outside a script passes.
    for (text, _) in both() {
        assert_clean(injection_problems(&text));
        let text = replaced(&text, FMT_STEP, "      - run: echo \"${{ github.ref_name }}\"\n");
        assert_problem(&injection_problems(&text), "expression in a run script", "fixture");
    }
}

#[test]
fn a_workflow_without_run_scripts_is_reported_in_a_fixture() {
    let none = steps_doc("      - uses: a/b@x");
    assert_eq!(run_scripts(&none).len(), 0);
    assert_eq!(injection_problems(&none), ["no run scripts found"]);
    // With nothing required, a workflow without scripts is still reported.
    assert_eq!(missing_commands(&none, &[]), ["no run scripts found"]);
    let required = missing_commands(&none, &["cargo test --workspace"]);
    assert_eq!(
        required,
        ["no run scripts found", "missing: cargo test --workspace"]
    );
}

// ---- required commands ----

#[test]
#[rustfmt::skip]
fn a_missing_command_is_caught_in_a_fixture() {
    let wanted = ["cargo test --workspace", "cargo fmt --all --check"];
    let only_comment = format!("# cargo test --workspace\n{}", steps_doc("      - run: cargo fmt --all --check"));
    assert_eq!(missing_commands(&only_comment, &wanted), ["missing: cargo test --workspace"]);
    // A command that only appears in a `name:` is not run.
    let named = steps_doc("      - name: cargo test --workspace\n        run: echo hello");
    assert_eq!(missing_commands(&named, &wanted[..1]), ["missing: cargo test --workspace"]);
    // A command only in a comment inside a block script.
    let steps = "      - run: |\n          # cargo test --workspace\n          cargo fmt --all --check # cargo clippy";
    let all = ["cargo fmt --all --check", "cargo test --workspace", "cargo clippy"];
    let missing = ["missing: cargo test --workspace", "missing: cargo clippy"];
    assert_eq!(missing_commands(&steps_doc(steps), &all), missing);
    // The real fixtures run each required command; losing one is reported.
    for (text, _) in both() {
        assert_clean(missing_commands(&text, REQUIRED_COMMANDS));
    }
    let no_tauri_install = missing_commands(&replaced(&good(), TAURI_INSTALL, ""), REQUIRED_COMMANDS);
    assert_eq!(no_tauri_install, ["missing: cargo install tauri-cli --locked --version 2.12.4"]);
}

#[test]
#[rustfmt::skip]
fn commands_in_one_line_and_block_scripts_are_found_in_a_fixture() {
    let steps = "      - run: cargo test --workspace\n      - name: x\n        shell: bash\n        run: |\n          node --test \"tests/ui/*.test.js\"";
    assert_clean(missing_commands(&steps_doc(steps), &["cargo test --workspace", "node --test \"tests/ui/*.test.js\""]));
}

#[test]
fn rust_tests_that_stop_at_the_first_failing_binary_are_reported_in_a_fixture() {
    let steps = "      - run: cargo test --workspace";
    let problems = missing_commands(&steps_doc(steps), REQUIRED_COMMANDS);
    assert!(
        problems.contains(&"missing: cargo test --workspace --no-fail-fast".to_owned()),
        "{problems:?}"
    );
    let steps = "      - run: cargo test --workspace --no-fail-fast";
    assert!(
        missing_commands(&steps_doc(steps), REQUIRED_COMMANDS)
            .iter()
            .all(|problem| !problem.contains("cargo test"))
    );
}

// The window script tests run even when the Rust tests failed, so one run
// reports the failures of both.
#[test]
#[rustfmt::skip]
fn a_node_step_that_runs_after_a_failed_step_passes_and_one_that_does_not_is_caught_in_a_fixture() {
    for (text, _) in both() {
        assert_clean(node_step_problems(&text));
    }
    let block = "      - name: x\n        if: success() || failure()\n        run: |\n          node --test \"tests/ui/*.test.js\"";
    assert_clean(node_step_problems(&steps_doc(block)));
    let texts = [
        // No condition: skipped once a step before it failed.
        with(NODE_STEP, "      - run: node --test \"tests/ui/*.test.js\"\n"),
        // Only after success.
        with(NODE_STEP, "      - run: node --test \"tests/ui/*.test.js\"\n        if: success()\n"),
        // The condition only in a comment.
        with(NODE_STEP, "      - run: node --test \"tests/ui/*.test.js\"\n        # if: success() || failure()\n"),
        with_mac(NODE_STEP, "      - run: node --test \"tests/ui/*.test.js\"\n"),
    ];
    assert_all_caught(node_step_problems, "a node test step without", &texts);
}

// The Rust tests run even after a failed formatting or lint step, so one run
// reports every failure on its platform.
#[test]
#[rustfmt::skip]
fn rust_tests_that_run_after_a_failed_step_pass_and_ones_that_do_not_are_caught_in_a_fixture() {
    for (text, _) in both() {
        assert_clean(rust_test_problems(&text));
    }
    let plain = "      - run: cargo test --workspace --no-fail-fast\n";
    let texts = [
        with(RUST_TESTS, plain),
        with(RUST_TESTS, &format!("{plain}        if: success()\n")),
        with(RUST_TESTS, &format!("{plain}        # if: success() || failure()\n")),
        with_mac(RUST_TESTS, plain),
        // A second cargo test step without the condition.
        add_step("      - run: cargo test --workspace"),
    ];
    assert_all_caught(rust_test_problems, "a cargo test step without", &texts);
    // No cargo test step at all, and one only in a comment.
    assert_problem(&rust_test_problems(&with(RUST_TESTS, "")), "no cargo test step", "none");
    let commented = with(RUST_TESTS, "      - run: |\n          # cargo test --workspace\n          ls\n");
    assert_problem(&rust_test_problems(&commented), "no cargo test step", "comment");
}

// ---- structure ----

#[test]
#[rustfmt::skip]
fn a_runner_that_is_not_the_jobs_runner_is_caught_in_a_fixture() {
    let one = structure_problems(&with("runs-on: windows-latest", "runs-on: ubuntu-latest"), &WINDOWS);
    assert_eq!(one.len(), 1, "{one:?}");
    assert_problem(&one, "job test: runner is not windows-latest", "ubuntu");
    assert_eq!(structure_problems(&good().replace("windows-latest", "ubuntu-latest"), &WINDOWS).len(), 2);
    let list = structure_problems(&with("runs-on: windows-latest", "runs-on: [windows-latest]"), &WINDOWS);
    assert_problem(&list, "job test: runs-on is not", "list");
    // Each Mac job has its own runner: the Intel job is on macos-15-intel, the others on macos-latest.
    let wrong_intel = in_job(&good_mac(), "build-intel", "runs-on: macos-15-intel", "runs-on: macos-latest");
    assert_eq!(structure_problems(&wrong_intel, &MACOS), ["job build-intel: runner is not macos-15-intel: macos-latest"]);
    let wrong_arm = in_job(&good_mac(), "build", "runs-on: macos-latest", "runs-on: macos-15-intel");
    assert_eq!(structure_problems(&wrong_arm, &MACOS), ["job build: runner is not macos-latest: macos-15-intel"]);
    let old = with_mac("runs-on: macos-latest", "runs-on: macos-13");
    assert_eq!(structure_problems(&old, &MACOS), ["job test: runner is not macos-latest: macos-13"]);
}

#[test]
#[rustfmt::skip]
fn a_job_without_runs_on_is_caught_in_a_fixture() {
    // Checked per job: the other job still has its runner.
    let one = with("  test:\n    runs-on: windows-latest\n", "  test:\n");
    assert_eq!(structure_problems(&one, &WINDOWS), ["job test: no runs-on"]);
    assert_eq!(structure_problems(&good().replace("    runs-on: windows-latest\n", ""), &WINDOWS).len(), 2);
}

#[test]
fn an_installer_job_without_needs_test_is_caught_in_a_fixture() {
    assert_eq!(
        structure_problems(&with("    needs: test\n", ""), &WINDOWS),
        ["the build job does not have needs: test"]
    );
    // On the Mac each Installer job is checked: dropping it from the second one names that job.
    let intel = in_job(&good_mac(), "build-intel", "    needs: test\n", "");
    assert_eq!(
        structure_problems(&intel, &MACOS),
        ["the build-intel job does not have needs: test"]
    );
}

#[test]
fn needs_test_on_the_wrong_job_is_caught_in_a_fixture() {
    // `needs: test` is on some line, but the build job does not have it.
    let text = with("    needs: test\n", "").replacen("  test:\n", "  test:\n    needs: test\n", 1);
    assert!(text.contains("needs: test"));
    assert_eq!(
        structure_problems(&text, &WINDOWS),
        ["the build job does not have needs: test"]
    );
    // A list that holds the job is fine, a list without it is not.
    assert_clean(structure_problems(
        &with("needs: test", "needs: [test]"),
        &WINDOWS,
    ));
    assert_eq!(
        structure_problems(&with("needs: test", "needs: [lint]"), &WINDOWS).len(),
        1
    );
}

#[test]
fn a_renamed_missing_or_extra_job_is_caught_in_a_fixture() {
    let renamed = good()
        .replace("  test:\n", "  checks:\n")
        .replace("needs: test", "needs: checks");
    assert_problem(
        &structure_problems(&renamed, &WINDOWS),
        "no test job",
        "renamed",
    );
    assert_problem(
        &structure_problems(&renamed, &WINDOWS),
        "unexpected job checks",
        "renamed",
    );
    let package = good().replace("  build:\n", "  package:\n");
    assert_problem(
        &structure_problems(&package, &WINDOWS),
        "no build job",
        "renamed build",
    );
    // The Mac needs both Installer jobs, and no job beside them.
    let no_intel = good_mac().replace("  build-intel:\n", "  other:\n");
    assert_problem(
        &structure_problems(&no_intel, &MACOS),
        "no build-intel job",
        "renamed intel",
    );
    assert_problem(
        &structure_problems(&no_intel, &MACOS),
        "unexpected job other",
        "renamed intel",
    );
    let extra = format!(
        "{}  deploy:\n    needs: test\n    runs-on: windows-latest\n    steps:\n      - run: ls\n",
        good()
    );
    assert_eq!(
        structure_problems(&extra, &WINDOWS),
        ["unexpected job deploy"]
    );
}

#[test]
fn an_installer_job_without_an_artifact_upload_is_caught_in_a_fixture() {
    let none = with("actions/upload-artifact@", "actions/download-artifact@");
    assert_eq!(structure_problems(&none, &WINDOWS).len(), 1);
    // An upload in the test job does not count for the build job.
    let wrong_job = with(UPLOAD_STEP, "").replacen(
        FMT_STEP,
        &format!("{FMT_STEP}{UPLOAD_STEP}").replace("PIN", PIN),
        1,
    );
    assert_eq!(structure_problems(&wrong_job, &WINDOWS).len(), 1);
    // The Intel job's own upload is needed too.
    let intel = in_job(
        &good_mac(),
        "build-intel",
        "actions/upload-artifact@",
        "actions/download-artifact@",
    );
    assert_eq!(
        structure_problems(&intel, &MACOS),
        ["the build-intel job does not upload an artifact"]
    );
}

#[test]
#[rustfmt::skip]
fn a_missing_concurrency_or_a_group_without_the_workflow_name_is_caught_in_a_fixture() {
    for (text, platform) in both() {
        let none = replaced(&text, "concurrency:\n  group: ${{ github.workflow }}-${{ github.ref }}\n", "");
        assert_eq!(structure_problems(&none, platform), ["no concurrency"]);
        let group = replaced(&text, "${{ github.workflow }}-", "x-");
        assert_problem(&structure_problems(&group, platform), "the concurrency group", "group");
    }
}

#[test]
#[rustfmt::skip]
fn a_windows_installer_job_that_restores_a_cache_is_caught_in_a_fixture() {
    for cache in ["Swatinem/rust-cache@PIN # v2", "actions/cache@PIN # v4", "actions/cache/restore@v4", "actions/cache/save@PIN # v4"] {
        let text = with(BUILD_STEP, &format!("{BUILD_STEP}      - uses: {cache}\n"));
        assert_problem(&structure_problems(&text, &WINDOWS), "the build job restores a cache", cache);
    }
}

#[test]
#[rustfmt::skip]
fn a_release_flag_and_a_release_path_in_a_run_script_pass_a_fixture() {
    for ok in [
        "      - run: cargo build --release", "      - run: cargo tauri build --release", "      - run: 'cargo build \"--release\"'",
        "      - run: ./target/release/video-optimizer --version", "      - run: ls \"target/release/bundle\"",
    ] {
        assert_clean(structure_problems(&add_step(ok), &WINDOWS));
    }
}

#[test]
#[rustfmt::skip]
fn a_release_word_that_is_not_the_flag_or_the_path_is_caught_in_a_fixture() {
    let bad = [
        "      - run: cargo build --release-notes", "      - run: cargo build --RELEASE", "      - run: curl x/target/release/y",
        "      - run: echo target/release/;gh release create v1", "      - run: gh release create v1 --release",
        "      - run: ls target/release/x; curl $U/releases",
    ];
    assert_all_caught(|text| structure_problems(text, &WINDOWS), "release step", &bad.map(add_step));
}

#[test]
#[rustfmt::skip]
fn a_release_step_is_caught_in_a_fixture() {
    let steps = [
        "      - uses: softprops/action-gh-release@PIN # v2", "      - uses: actions/create-release@PIN # v1",
        "      - uses: ncipollo/release-action@PIN # v1", "      - uses: actions/upload-release-asset@PIN # v1",
        "      - uses: acme/release/pub@PIN # v1", "      - uses: tauri-apps/tauri-action@PIN # v0",
        "      - run: gh release create v1 --notes x", "      -  run: gh release create v1", "      - run: \"gh release create v1\"",
        "      - run: 'echo \"a #b\"; gh release create v1'",
        "      - run: |\n          echo start\n          echo \"a #b\"; gh release upload v1 a.exe",
        "      - run: |\n          # gh release create v1",
        "      - run: gh api repos/o/r/releases -f tag_name=v1", "      - run: curl -X POST https://example.com/releases",
    ];
    assert_all_caught(|text| structure_problems(text, &WINDOWS), "release step", &steps.map(add_step));
    // A job that calls a reusable release workflow.
    let job = format!("{}  publish:\n    uses: org/repo/.github/workflows/release.yml@{PIN} # v1\n", good());
    assert_problem(&structure_problems(&job, &WINDOWS), "release step", "reusable workflow");
    // The same in the Mac fixture.
    let mac = format!("{}      - run: gh release create v1\n", good_mac());
    assert_problem(&structure_problems(&mac, &MACOS), "release step", "mac");
}

#[test]
#[rustfmt::skip]
fn a_release_id_input_is_caught_in_a_fixture() {
    let bad = [
        "      - uses: a/b@PIN # v1\n        with:\n          releaseId: 1",
        "      - run: ls\n        env:\n          RELEASE_ID: 1",
        "      - run: echo $releaseId",
    ];
    assert_all_caught(|text| structure_problems(text, &WINDOWS), "release step", &bad.map(add_step));
}

// ---- settings ----

#[test]
fn settings_that_are_right_pass_a_fixture() {
    // Quoting does not change a value.
    for version in ["'24.1'", "\"22\"", "'22'", "22"] {
        assert_clean(setting_problems(&with(
            "node-version: 22",
            &format!("node-version: {version}"),
        )));
    }
    assert_clean(upload_problems(
        &with("if-no-files-found: error", "if-no-files-found: 'error'"),
        &WINDOWS,
    ));
}

#[test]
#[rustfmt::skip]
fn a_shell_other_than_bash_or_no_defaults_is_caught_in_a_fixture() {
    assert_eq!(setting_problems(&with("shell: bash", "shell: pwsh")).len(), 1);
    assert_eq!(setting_problems(&with("defaults:\n  run:\n    shell: bash\n", "")).len(), 1);
    assert_eq!(setting_problems(&with("  run:\n    shell: bash\n", "  run: {}\n")).len(), 1);
}

#[test]
#[rustfmt::skip]
fn an_old_or_unreadable_node_version_is_caught_in_a_fixture() {
    for bad in ["20", "'21.9'", "'lts/*'", "latest"] {
        assert_eq!(setting_problems(&with("node-version: 22", &format!("node-version: {bad}"))).len(), 1, "{bad}");
    }
    assert_eq!(setting_problems(&with("        with:\n          node-version: 22\n", "")), ["no node-version"]);
}

// ---- the uploads ----

#[test]
#[rustfmt::skip]
fn an_upload_that_ignores_missing_files_is_caught_in_a_fixture() {
    for value in ["warn", "ignore", "'warn'"] {
        let text = with("if-no-files-found: error", &format!("if-no-files-found: {value}"));
        assert_eq!(upload_problems(&text, &WINDOWS).len(), 1, "{value}");
    }
    // The action's default is to warn, so an upload that sets nothing is wrong.
    assert_eq!(upload_problems(&with("          if-no-files-found: error\n", ""), &WINDOWS).len(), 1);
    assert_eq!(upload_problems(&with(UPLOAD_STEP, ""), &WINDOWS).len(), 1);
    // A step that is not an upload but ignores missing files.
    let other = "      - uses: other/action@PIN # v1\n        with:\n          if-no-files-found: warn\n";
    assert_problem(&upload_problems(&add_step(other), &WINDOWS), "if-no-files-found is warn", "other action");
    let fine = "      - uses: other/action@PIN # v1\n        with:\n          if-no-files-found: error\n";
    assert_clean(upload_problems(&add_step(fine), &WINDOWS));
    // Set, but to a list, a null or a mapping: not "does not set".
    for value in ["[error]", "null", "{a: error}"] {
        let text = with("if-no-files-found: error", &format!("if-no-files-found: {value}"));
        let problems = upload_problems(&text, &WINDOWS);
        assert_eq!(problems.len(), 1, "{value}: {problems:?}");
        assert_problem(&problems, "if-no-files-found is not a single value", value);
    }
    let missing = upload_problems(&with("          if-no-files-found: error\n", ""), &WINDOWS);
    assert_eq!(missing, ["an upload does not set if-no-files-found"]);
    // In the Mac fixture, in the second job only.
    let intel = in_job(&good_mac(), "build-intel", "if-no-files-found: error", "if-no-files-found: warn");
    assert_eq!(upload_problems(&intel, &MACOS), ["if-no-files-found is warn, not error"]);
}

#[test]
#[rustfmt::skip]
fn the_upload_names_the_job_artifact_its_paths_and_thirty_days_in_a_fixture() {
    for (text, platform) in both() {
        assert_clean(upload_problems(&text, platform));
    }
    // The paths are a set of trimmed lines: order, trailing spaces, blank
    // lines, tabs, Windows line endings, `|-` and `|+` change nothing.
    let one = "path: target/release/bundle/nsis/*.exe\n";
    assert_clean(upload_problems(&with(one, "path: |\n            target/release/bundle/nsis/*.exe   \n\n"), &WINDOWS));
    assert_clean(upload_problems(&with(one, "path: |-\n            target/release/bundle/nsis/*.exe\n"), &WINDOWS));
    assert_clean(upload_problems(&with(one, "path: |+\n            target/release/bundle/nsis/*.exe\n"), &WINDOWS));
    assert_clean(upload_problems(&good().replace('\n', "\r\n"), &WINDOWS));
    // The name may be quoted; the retention is the number 30 however it is
    // written: the parser reads `30`, `030`, `+30`, `0x1E` and `0o36` as the
    // integer 30 and `"30"` as a string. A quoted `'030'` is the text 030.
    for name in ["'video-optimizer-windows-installer'", "\"video-optimizer-windows-installer\""] {
        assert_clean(upload_problems(&with("name: video-optimizer-windows-installer", &format!("name: {name}")), &WINDOWS));
    }
    for days in ["'30'", "\"30\"", "30", "030", "+30", "0x1E", "0o36"] {
        assert_clean(upload_problems(&with("retention-days: 30", &format!("retention-days: {days}")), &WINDOWS));
    }
}

#[test]
#[rustfmt::skip]
fn an_upload_with_another_name_path_or_retention_is_caught_in_a_fixture() {
    let name = upload_problems(&with("name: video-optimizer-windows-installer", "name: video-optimizer"), &WINDOWS);
    assert_eq!(name.len(), 1, "{name:?}");
    assert_problem(&name, "an upload is named video-optimizer, not video-optimizer-windows-installer", "name");
    let other = with("path: target/release/bundle/nsis/*.exe\n", "path: |\n            target/release/bundle/nsis/*.exe\n            target/release/bundle/msi/*.msi\n");
    assert_problem(&upload_problems(&other, &WINDOWS), "an upload's paths are", "extra path");
    let wrong = with("path: target/release/bundle/nsis/*.exe\n", "path: target/release/bundle/msi/*.msi\n");
    assert_problem(&upload_problems(&wrong, &WINDOWS), "an upload's paths are", "other path");
    let days = upload_problems(&with("retention-days: 30", "retention-days: 90"), &WINDOWS);
    assert_eq!(days.len(), 1, "{days:?}");
    assert_problem(&days, "an upload's retention-days is 90, not 30", "90");
    // The action's default retention is 90 days, so an upload that sets nothing is wrong.
    let none = upload_problems(&with("          retention-days: 30\n", ""), &WINDOWS);
    assert_eq!(none, ["an upload does not set retention-days"]);
}

#[test]
#[rustfmt::skip]
fn an_upload_with_a_shape_the_rule_does_not_expect_is_caught_in_a_fixture() {
    for days in ["'90'", "29", "31", "3", "300", "30.0", "'30 '", "'30d'", "[30]", "{a: 30}", "'030'", "\"030\"", "0", "''", "null"] {
        let text = with("retention-days: 30", &format!("retention-days: {days}"));
        assert_eq!(upload_problems(&text, &WINDOWS).len(), 1, "retention-days: {days}");
    }
    for name in ["video-optimizer-windows-installer-2", "Video-Optimizer-Windows-Installer", "'video-optimizer-windows-installer '", "[video-optimizer-windows-installer]", "''", "null"] {
        let text = with("name: video-optimizer-windows-installer", &format!("name: {name}"));
        assert_eq!(upload_problems(&text, &WINDOWS).len(), 1, "name: {name}");
    }
    let no_name = with("          name: video-optimizer-windows-installer\n", "");
    assert_problem(&upload_problems(&no_name, &WINDOWS), "an upload does not set a name", "no name");
    let one_line = with("path: target/release/bundle/nsis/*.exe\n", "path: >\n            target/release/bundle/nsis/*.exe\n            target/release/bundle/msi/*.msi\n");
    assert_problem(&upload_problems(&one_line, &WINDOWS), "an upload's paths are", "folded into one line");
    let list = with("path: target/release/bundle/nsis/*.exe\n", "path:\n            - target/release/bundle/nsis/*.exe\n");
    assert_problem(&upload_problems(&list, &WINDOWS), "an upload's path is not", "a YAML list");
    let no_path = with("          path: target/release/bundle/nsis/*.exe\n", "          x: y\n");
    assert_problem(&upload_problems(&no_path, &WINDOWS), "an upload does not set a path", "no path");
    let negated = with("path: target/release/bundle/nsis/*.exe\n", "path: |\n            target/release/bundle/nsis/*.exe\n            !target/release/bundle/nsis/x.exe\n");
    assert_problem(&upload_problems(&negated, &WINDOWS), "an upload's paths are", "exclusion line");
    let bare = with("        with:\n          name: video-optimizer-windows-installer\n          path: target/release/bundle/nsis/*.exe\n          if-no-files-found: error\n          retention-days: 30\n", "");
    assert_eq!(upload_problems(&bare, &WINDOWS).len(), 4, "{:?}", upload_problems(&bare, &WINDOWS));
}

#[test]
fn every_upload_in_the_workflow_is_checked_and_belongs_to_an_installer_job_in_a_fixture() {
    // A second upload in the build job under another name.
    let upload = UPLOAD_STEP.replace("video-optimizer-windows-installer", "video-optimizer");
    assert_problem(
        &upload_problems(&add_step(&upload), &WINDOWS),
        "an upload is named video-optimizer",
        "a second upload",
    );
    // The same upload twice is fine.
    assert_clean(upload_problems(&add_step(UPLOAD_STEP), &WINDOWS));
    // An upload in the test job is not an installer's.
    let in_test = with(FMT_STEP, &format!("{FMT_STEP}{UPLOAD_STEP}"));
    assert_problem(
        &upload_problems(&in_test, &WINDOWS),
        "an upload in job test, which is not an installer job",
        "upload in test",
    );
}

#[test]
fn a_workflow_without_an_upload_is_reported_by_the_upload_check_in_a_fixture() {
    let none = with("actions/upload-artifact@", "actions/download-artifact@");
    assert_problem(
        &upload_problems(&none, &WINDOWS),
        "no upload-artifact step in job build",
        "download instead",
    );
    // On the Mac each job has its own: only the Intel job lost its upload.
    let intel = in_job(
        &good_mac(),
        "build-intel",
        "actions/upload-artifact@",
        "actions/download-artifact@",
    );
    assert_eq!(
        upload_problems(&intel, &MACOS),
        ["no upload-artifact step in job build-intel"]
    );
}

#[test]
fn each_mac_installer_job_uploads_its_own_artifact_in_a_fixture() {
    assert_clean(upload_problems(&good_mac(), &MACOS));
    // The Intel job under the Apple Silicon name, and the other way round.
    let intel = in_job(
        &good_mac(),
        "build-intel",
        "video-optimizer-macos-intel-dmg",
        "video-optimizer-macos-arm64-dmg",
    );
    assert_eq!(
        upload_problems(&intel, &MACOS),
        ["an upload is named video-optimizer-macos-arm64-dmg, not video-optimizer-macos-intel-dmg"]
    );
    let arm = in_job(
        &good_mac(),
        "build",
        "video-optimizer-macos-arm64-dmg",
        "video-optimizer-macos-intel-dmg",
    );
    assert_eq!(
        upload_problems(&arm, &MACOS),
        ["an upload is named video-optimizer-macos-intel-dmg, not video-optimizer-macos-arm64-dmg"]
    );
    // The Windows artifact name is wrong for a Mac job.
    let windows = with_mac(
        "video-optimizer-macos-arm64-dmg",
        "video-optimizer-windows-installer",
    );
    assert_problem(
        &upload_problems(&windows, &MACOS),
        "an upload is named video-optimizer-windows-installer",
        "windows name",
    );
}

// ---- the platform profile ----

/// A profile that differs from `WINDOWS` in its file, runners and artifact.
/// Its cache and `save-if` policies are Windows's.
const OTHER: Platform = Platform {
    file: "other.yml",
    test_runner: "other-runner",
    installers: &[Installer {
        job: "build",
        runner: "other-runner",
        artifact: "other-artifact",
        artifact_paths: &["out/*.pkg"],
        arch: None,
    }],
    ..WINDOWS
};

#[test]
fn the_runner_comes_from_the_profile_in_a_fixture() {
    let other = good().replace("windows-latest", "other-runner");
    assert_clean(structure_problems(&other, &OTHER));
    // Under the other profile the Windows runner is the wrong one, in both jobs,
    // and under the Windows profile the other runner is.
    let wrong = structure_problems(&good(), &OTHER);
    assert_eq!(wrong.len(), 2, "{wrong:?}");
    assert_problem(
        &wrong,
        "job test: runner is not other-runner",
        "windows under other",
    );
    assert_eq!(structure_problems(&other, &WINDOWS).len(), 2);
}

#[test]
fn the_artifact_and_its_paths_come_from_the_profile_in_a_fixture() {
    let other = with(
        "name: video-optimizer-windows-installer",
        "name: other-artifact",
    )
    .replace("target/release/bundle/nsis/*.exe", "out/*.pkg");
    assert_clean(upload_problems(&other, &OTHER));
    let wrong = upload_problems(&good(), &OTHER);
    assert_eq!(wrong.len(), 2, "{wrong:?}");
    assert_problem(
        &wrong,
        "an upload is named video-optimizer-windows-installer, not other-artifact",
        "name",
    );
    assert_problem(&wrong, "an upload's paths are", "paths");
    assert_eq!(upload_problems(&other, &WINDOWS).len(), 2);
    // Only the name is the other profile's: the paths alone are wrong.
    let name_only = with(
        "name: video-optimizer-windows-installer",
        "name: other-artifact",
    );
    let paths = upload_problems(&name_only, &OTHER);
    assert_eq!(paths.len(), 1, "{paths:?}");
    assert_problem(&paths, "an upload's paths are", "paths only");
    // Only the paths are the other profile's: the name alone is wrong.
    let paths_only = with("target/release/bundle/nsis/*.exe", "out/*.pkg");
    let name = upload_problems(&paths_only, &OTHER);
    assert_eq!(name.len(), 1, "{name:?}");
    assert_problem(
        &name,
        "an upload is named video-optimizer-windows-installer",
        "name only",
    );
}

#[test]
#[should_panic(expected = "other.yml")]
fn the_workflow_file_comes_from_the_profile() {
    // `workflow(&WINDOWS)` reads windows.yml in the real-file tests; a profile
    // that names a file that does not exist cannot be read.
    workflow(&OTHER);
}

// ---- the bundle build ----

#[test]
#[rustfmt::skip]
fn a_bundle_build_with_the_config_file_passes_a_fixture() {
    for (text, platform) in both() {
        assert_clean(bundle_build_problems(&text, platform));
    }
    for step in [
        "      - run: cargo tauri build --config=src-tauri/tauri.bundle.conf.json\n",
        "      - run: cargo tauri build --verbose --config 'src-tauri/tauri.bundle.conf.json'\n",
        "      - run: |\n          cargo --version\n          cargo tauri build --config \"src-tauri/tauri.bundle.conf.json\"\n",
    ] {
        assert_clean(bundle_build_problems(&with(BUILD_STEP, step), &WINDOWS));
    }
}

#[test]
#[rustfmt::skip]
fn a_bundle_build_without_the_config_file_is_caught_in_a_fixture() {
    let plain = "      - run: cargo tauri build\n";
    let without = "a bundle build without --config";
    let texts = [
        // The plain build.
        with(BUILD_STEP, plain),
        // The flag only in a shell comment.
        with(BUILD_STEP, "      - run: |\n          cargo tauri build\n          # --config src-tauri/tauri.bundle.conf.json\n"),
        with(BUILD_STEP, "      - run: |\n          cargo tauri build # --config src-tauri/tauri.bundle.conf.json\n"),
        // Another file, or the flag without its file.
        with(BUILD_STEP, "      - run: cargo tauri build --config src-tauri/tauri.conf.json\n"),
        with(BUILD_STEP, "      - run: cargo tauri build --config\n"),
        // The flag only in the test job.
        with(BUILD_STEP, plain).replacen(FMT_STEP, &format!("{FMT_STEP}{BUILD_STEP}"), 1),
        // A second, plain build after the right one.
        add_step("      - run: cargo tauri build"),
    ];
    assert_all_caught(|text| bundle_build_problems(text, &WINDOWS), without, &texts);
    // The Mac: only the Intel job's build lost the flag.
    let intel = in_job(&good_mac(), "build-intel", BUILD_STEP, plain);
    assert_eq!(bundle_build_problems(&intel, &MACOS).len(), 1);
    assert_problem(&bundle_build_problems(&intel, &MACOS), "a bundle build without --config src-tauri/tauri.bundle.conf.json in job build-intel", "intel");
    // No bundle build in the build job at all, and no build job.
    let none = with(BUILD_STEP, "      - run: cargo build --release\n");
    assert_problem(&bundle_build_problems(&none, &WINDOWS), "the build job has no cargo tauri build", &none);
    let no_build = good().replacen("  build:\n", "  installer:\n", 1);
    assert_problem(&bundle_build_problems(&no_build, &WINDOWS), "no build job", &no_build);
}

// ---- ffmpeg for the tests ----

#[test]
#[rustfmt::skip]
fn the_test_job_that_stages_ffmpeg_and_exports_the_folder_before_the_tests_passes_a_fixture() {
    for (text, _) in both() {
        assert_clean(tools_dir_problems(&text));
    }
    // Two steps, the command and the export apart: still before the tests.
    let apart = "      - run: cargo run --locked -p video-optimizer-xtask -- ffmpeg --dest \"$RUNNER_TEMP/ffmpeg\"\n      - run: echo \"VIDEO_OPTIMIZER_TOOLS_DIR=$RUNNER_TEMP/ffmpeg\" >> \"$GITHUB_ENV\"\n";
    assert_clean(tools_dir_problems(&with(STAGE_STEP, apart)));
    // The installer jobs stage through `beforeBuildCommand`: nothing is asked of them.
    assert_clean(tools_dir_problems(&with(BUILD_STEP, &format!("{BUILD_STEP}      - run: ls\n"))));
}

#[test]
#[rustfmt::skip]
fn a_test_job_that_does_not_stage_ffmpeg_for_the_tests_is_caught_in_a_fixture() {
    let stage = "cargo run --locked -p video-optimizer-xtask -- ffmpeg --dest \"$RUNNER_TEMP/ffmpeg\"";
    let export = "echo \"VIDEO_OPTIMIZER_TOOLS_DIR=$RUNNER_TEMP/ffmpeg\" >> \"$GITHUB_ENV\"";
    let step = |body: &str| format!("      - run: |\n          {}\n", body.replace('\n', "\n          "));
    let no_stage = step(export);
    assert_problem(&tools_dir_problems(&with(STAGE_STEP, &no_stage)), "the test job does not stage ffmpeg", "no staging");
    let no_export = step(stage);
    assert_problem(&tools_dir_problems(&with(STAGE_STEP, &no_export)), "the test job does not set VIDEO_OPTIMIZER_TOOLS_DIR", "no export");
    // The variable set in a place the later steps do not read, another folder, another name.
    for export in [
        "echo \"VIDEO_OPTIMIZER_TOOLS_DIR=$RUNNER_TEMP/ffmpeg\"",
        "echo \"VIDEO_OPTIMIZER_TOOLS_DIR=$RUNNER_TEMP/other\" >> \"$GITHUB_ENV\"",
        "echo \"VIDEO_OPTIMIZER_TOOL_DIR=$RUNNER_TEMP/ffmpeg\" >> \"$GITHUB_ENV\"",
        "echo \"VIDEO_OPTIMIZER_TOOLS_DIR=$RUNNER_TEMP/ffmpeg\" >> \"$GITHUB_PATH\"",
    ] {
        let text = with(STAGE_STEP, &step(&format!("{stage}\n{export}")));
        assert_problem(&tools_dir_problems(&text), "the test job does not set VIDEO_OPTIMIZER_TOOLS_DIR", export);
    }
    // The staging to another folder, without --dest, or for the sidecars only.
    for stage in [
        "cargo run --locked -p video-optimizer-xtask -- ffmpeg --dest \"$RUNNER_TEMP/other\"",
        "cargo run --locked -p video-optimizer-xtask -- ffmpeg",
        "cargo run -p video-optimizer-xtask -- ffmpeg --dest \"$RUNNER_TEMP/ffmpeg\"",
    ] {
        let text = with(STAGE_STEP, &step(&format!("{stage}\n{export}")));
        assert_problem(&tools_dir_problems(&text), "the test job does not stage ffmpeg", stage);
    }
    // Only in a shell comment.
    let commented = step(&format!("# {stage}\n{export}"));
    assert_problem(&tools_dir_problems(&with(STAGE_STEP, &commented)), "the test job does not stage ffmpeg", "comment");
    // After the tests: the variable is read by the tests only if it is set before them.
    let late_stage = with(STAGE_STEP, "").replacen(RUST_TESTS, &format!("{RUST_TESTS}{}", step(stage)), 1);
    assert_problem(&tools_dir_problems(&late_stage), "the test job stages ffmpeg after its first cargo test", "late stage");
    let late_export = with(STAGE_STEP, &step(stage)).replacen(RUST_TESTS, &format!("{RUST_TESTS}{}", step(export)), 1);
    assert_problem(&tools_dir_problems(&late_export), "the test job sets VIDEO_OPTIMIZER_TOOLS_DIR after its first cargo test", "late export");
    // The same in the Mac fixture, and no tests in the test job.
    assert_problem(&tools_dir_problems(&with_mac(STAGE_STEP, "")), "the test job does not stage ffmpeg", "mac");
    assert_problem(&tools_dir_problems(&with(RUST_TESTS, "")), "no cargo test step in the test job", "no tests");
    assert_eq!(tools_dir_problems(&good().replace("  test:\n", "  checks:\n")), ["no test job"]);
}

// ---- the installer checks: Windows ----

#[test]
fn a_windows_check_step_between_the_build_and_the_upload_passes_a_fixture() {
    assert_clean(installer_check_problems(&good(), &WINDOWS));
}

#[test]
#[rustfmt::skip]
fn a_windows_check_step_that_is_missing_incomplete_or_out_of_order_is_caught_in_a_fixture() {
    let none = with(WINDOWS_CHECK_STEP, "");
    assert_eq!(installer_check_problems(&none, &WINDOWS), ["job build: no check step: no step tests the files beside the app"]);
    // Each file is needed: dropping one line at a time.
    for line in WINDOWS_CHECK_LINES {
        let text = with(&format!("          {line}\n"), "");
        assert_eq!(installer_check_problems(&text, &WINDOWS), [format!("job build: the check step does not run {line}")]);
    }
    // A line that does not test for the file: only listed, or only commented.
    let listed = with("          test -f target/release/ffprobe.exe\n", "          ls target/release/ffprobe.exe\n");
    assert_problem(&installer_check_problems(&listed, &WINDOWS), "job build: the check step does not run test -f target/release/ffprobe.exe", "listed");
    let commented = with("          test -f target/release/ffprobe.exe\n", "          # test -f target/release/ffprobe.exe\n");
    assert_problem(&installer_check_problems(&commented, &WINDOWS), "job build: the check step does not run test -f target/release/ffprobe.exe", "commented");
    // Out of order: before the build, and after the upload.
    let before = with(&format!("{BUILD_STEP}{WINDOWS_CHECK_STEP}"), &format!("{WINDOWS_CHECK_STEP}{BUILD_STEP}"));
    assert_problem(&installer_check_problems(&before, &WINDOWS), "job build: the check step does not come after a cargo tauri build", "before the build");
    let after = with(&format!("{WINDOWS_CHECK_STEP}{UPLOAD_STEP}"), &format!("{UPLOAD_STEP}{WINDOWS_CHECK_STEP}"));
    assert_problem(&installer_check_problems(&after, &WINDOWS), "job build: the check step does not come before the upload", "after the upload");
    // A check in the test job does not count for the build job.
    let wrong_job = with(WINDOWS_CHECK_STEP, "").replacen(FMT_STEP, &format!("{FMT_STEP}{WINDOWS_CHECK_STEP}"), 1);
    assert_eq!(installer_check_problems(&wrong_job, &WINDOWS), ["job build: no check step: no step tests the files beside the app"]);
}

// ---- the installer checks: Mac ----

#[test]
fn a_mac_check_step_between_each_build_and_its_upload_passes_a_fixture() {
    assert_clean(installer_check_problems(&good_mac(), &MACOS));
}

#[test]
#[rustfmt::skip]
fn a_mac_check_step_that_is_missing_or_commented_is_caught_in_each_job_in_a_fixture() {
    for (job, runner_arch) in [("build", "arm64"), ("build-intel", "x86_64")] {
        let step = MAC_CHECK_STEP.replace("@ARCH@", runner_arch);
        let none = in_job(&good_mac(), job, &step, "");
        assert_eq!(installer_check_problems(&none, &MACOS), [format!("job {job}: no check step: no step runs {CHECK_SCRIPT}")]);
        let commented = in_job(&good_mac(), job, "        run: bash scripts/check-mac-app.sh \"$EXPECTED_ARCH\"\n", "        run: |\n          # bash scripts/check-mac-app.sh \"$EXPECTED_ARCH\"\n          ls\n");
        assert_eq!(installer_check_problems(&commented, &MACOS), [format!("job {job}: no check step: no step runs {CHECK_SCRIPT}")]);
    }
}

#[test]
#[rustfmt::skip]
fn a_mac_check_step_that_is_not_exactly_the_script_call_is_caught_in_each_job_in_a_fixture() {
    let call = "        run: bash scripts/check-mac-app.sh \"$EXPECTED_ARCH\"\n";
    // The spelling of the argument does not matter.
    for ok in ["        run: bash scripts/check-mac-app.sh $EXPECTED_ARCH\n", "        run: 'bash scripts/check-mac-app.sh \"$EXPECTED_ARCH\"'\n"] {
        assert_clean(installer_check_problems(&in_job(&good_mac(), "build", call, ok), &MACOS));
    }
    // A call that can pass when the script fails, or checks something else.
    let wrong_call = [
        "        run: bash scripts/check-mac-app.sh \"$EXPECTED_ARCH\" || true\n",
        "        run: bash scripts/check-mac-app.sh\n",
        "        run: bash scripts/check-mac-app.sh arm64\n",
        "        run: bash scripts/check-mac-app.sh x86_64\n",
        "        run: sh scripts/check-mac-app.sh \"$EXPECTED_ARCH\"\n",
        "        run: bash scripts/check-mac-app.sh \"$EXPECTED_ARCH\" extra\n",
        "        run: bash scripts/check-mac-app.sh \"$EXPECTED_ARCH\"; true\n",
        "        run: |\n          bash scripts/check-mac-app.sh \"$EXPECTED_ARCH\"\n          exit 0\n",
        "        run: |\n          set +e\n          bash scripts/check-mac-app.sh \"$EXPECTED_ARCH\"\n",
    ];
    for job in ["build", "build-intel"] {
        for wrong in wrong_call {
            let text = in_job(&good_mac(), job, call, wrong);
            assert_problem(&installer_check_problems(&text, &MACOS), &format!("job {job}: the check step must run exactly"), wrong);
        }
        // A step that cannot fail the job, or may be skipped.
        for extra in ["        continue-on-error: true\n", "        if: success() || failure()\n"] {
            let text = in_job(&good_mac(), job, call, &format!("{call}{extra}"));
            assert_problem(&installer_check_problems(&text, &MACOS), &format!("job {job}: the check step has"), extra);
        }
    }
}

#[test]
#[rustfmt::skip]
fn the_expected_architecture_of_each_mac_job_comes_from_the_profile_in_a_fixture() {
    // The Intel job checking for arm64, the Apple Silicon job checking for x86_64, and none set.
    let intel = in_job(&good_mac(), "build-intel", "EXPECTED_ARCH: x86_64", "EXPECTED_ARCH: arm64");
    assert_eq!(installer_check_problems(&intel, &MACOS), ["job build-intel: the check step's EXPECTED_ARCH is Some(\"arm64\"), not x86_64"]);
    let arm = in_job(&good_mac(), "build", "EXPECTED_ARCH: arm64", "EXPECTED_ARCH: x86_64");
    assert_eq!(installer_check_problems(&arm, &MACOS), ["job build: the check step's EXPECTED_ARCH is Some(\"x86_64\"), not arm64"]);
    let unset = in_job(&good_mac(), "build", "      - env:\n          EXPECTED_ARCH: arm64\n        run: bash", "      - run: bash");
    assert_eq!(installer_check_problems(&unset, &MACOS), ["job build: the check step's EXPECTED_ARCH is None, not arm64"]);
    // A quoted value is the same value.
    let quoted = in_job(&good_mac(), "build", "EXPECTED_ARCH: arm64", "EXPECTED_ARCH: 'arm64'");
    assert_clean(installer_check_problems(&quoted, &MACOS));
}

#[test]
#[rustfmt::skip]
fn a_mac_check_step_out_of_order_is_caught_in_a_fixture() {
    let arm_check = MAC_CHECK_STEP.replace("@ARCH@", "arm64");
    let arm_upload = MAC_UPLOAD_STEP.replace("@ARTIFACT@", "video-optimizer-macos-arm64-dmg");
    // Before the build, and after the upload, in the Apple Silicon job.
    let before = in_job(&good_mac(), "build", &format!("{BUILD_STEP}{arm_check}"), &format!("{arm_check}{BUILD_STEP}"));
    assert_problem(&installer_check_problems(&before, &MACOS), "job build: the check step does not come after a cargo tauri build", "before the build");
    let after = in_job(&good_mac(), "build", &format!("{arm_check}{arm_upload}"), &format!("{arm_upload}{arm_check}"));
    assert_problem(&installer_check_problems(&after, &MACOS), "job build: the check step does not come before the upload", "after the upload");
    // A check in the test job does not count for the installer jobs.
    let wrong_job = in_job(&good_mac(), "build", &arm_check, "")
        .replacen(FMT_STEP, &format!("{FMT_STEP}{arm_check}"), 1);
    assert_eq!(installer_check_problems(&wrong_job, &MACOS), [format!("job build: no check step: no step runs {CHECK_SCRIPT}")]);
}

// ---- the cache saves only from main ----

#[test]
#[rustfmt::skip]
fn a_mac_cache_that_saves_only_from_main_passes_a_fixture() {
    assert_clean(cache_save_problems(&good_mac(), &MACOS));
    // Quoting and a block scalar read the same.
    let block = "        with:\n          save-if: >-\n            ${{ github.ref == 'refs/heads/main' }}\n";
    assert_clean(cache_save_problems(&with_mac(MAC_SAVE_IF, block), &MACOS));
    // Windows asks nothing of it: the same fixture shape is clean without a `save-if`.
    assert_clean(cache_save_problems(&good(), &WINDOWS));
}

#[test]
#[rustfmt::skip]
fn a_mac_cache_with_another_save_if_or_none_is_caught_in_each_job_in_a_fixture() {
    let wrong = [
        "true", "false", "${{ github.ref == 'refs/heads/develop' }}", "${{ github.ref=='refs/heads/main' }}",
        "${{ github.event_name == 'push' }}", "[true]", "null",
    ];
    for save_if in wrong {
        let text = with_mac(MAC_SAVE_IF, &format!("        with:\n          save-if: {save_if}\n"));
        assert_eq!(cache_save_problems(&text, &MACOS).len(), 1, "{save_if}");
        // The same in the second Installer job.
        let intel = in_job(&good_mac(), "build-intel", "save-if: ${{ github.ref == 'refs/heads/main' }}", &format!("save-if: {save_if}"));
        assert_eq!(cache_save_problems(&intel, &MACOS).len(), 1, "intel {save_if}");
    }
    let none = with_mac(MAC_SAVE_IF, "");
    assert_eq!(cache_save_problems(&none, &MACOS), ["a rust-cache step has no save-if"]);
    let none_intel = in_job(&good_mac(), "build-intel", MAC_BUILD_CACHE, "      - uses: Swatinem/rust-cache@PIN # v2.9.3\n");
    assert_eq!(cache_save_problems(&none_intel, &MACOS), ["a rust-cache step has no save-if"]);
    // No cache step at all.
    let nothing = steps_doc("      - run: ls");
    assert_eq!(cache_save_problems(&nothing, &MACOS), ["no rust-cache step to check"]);
}

#[test]
fn the_save_if_rule_comes_from_the_profile_in_a_fixture() {
    // The Windows fixture has no save-if: wrong for a profile that asks for one.
    let wrong = cache_save_problems(&good(), &MACOS);
    assert_eq!(wrong, ["a rust-cache step has no save-if"]);
    // The Mac fixture is wrong under a profile that asks for no value other than main's... but fine for one that asks nothing.
    assert_clean(cache_save_problems(&good_mac(), &WINDOWS));
}

// ---- the Mac Installer jobs restore their own cache ----

#[test]
#[rustfmt::skip]
fn a_mac_installer_job_without_its_cache_or_with_it_after_cargo_is_caught_in_a_fixture() {
    assert_clean(structure_problems(&good_mac(), &MACOS));
    for job in ["build", "build-intel"] {
        let missing = [format!("the {job} job does not restore Swatinem/rust-cache")];
        assert_eq!(structure_problems(&in_job(&good_mac(), job, MAC_BUILD_CACHE, ""), &MACOS), missing, "{job}");
        // Another cache action is not the one the job relies on.
        let other = in_job(&good_mac(), job, MAC_BUILD_CACHE, "      - uses: actions/cache@PIN # v4\n");
        assert_eq!(structure_problems(&other, &MACOS), missing, "{job}");
        // After the first cargo step (the Tauri install), so that install is never cached.
        let late = in_job(&good_mac(), job, &format!("{MAC_BUILD_CACHE}{TAURI_INSTALL}"), &format!("{TAURI_INSTALL}{MAC_BUILD_CACHE}"));
        assert_eq!(structure_problems(&late, &MACOS), [format!("the {job} job restores its cache after its first cargo step")], "{job}");
    }
    // The policy comes from the profile: the Windows shape (no Installer cache) is wrong for the
    // Mac, and the Mac shape is wrong for Windows.
    let forbidden = structure_problems(&good_mac().replace("macos-15-intel", "macos-latest"), &WINDOWS);
    assert_problem(&forbidden, "the build job restores a cache: Swatinem/rust-cache@", "Mac shape");
}
