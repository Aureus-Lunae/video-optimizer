//! Tests of `scripts/check-mac-app.sh`, the check both Mac Installer jobs of
//! `.github/workflows/macos.yml` run on the built app and on the app inside
//! the mounted disk image. The real tools (`codesign`, `lipo`, `vtool`,
//! `plutil`, `hdiutil`) exist only on a Mac, so each test puts stub programs
//! of the same names first on `PATH`; the stubs print what the real tools
//! print (or what a broken build would make them print) and log their calls.
//! The script is plain bash and awk, so these tests run on Linux too. They are
//! Unix only: the Windows runner has no use for the script.
//!
//! The workflow guard (`workflow.rs`) only proves that each job calls the
//! script with its own architecture; what the script refuses is proved here.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const APP: &str = "target/release/bundle/macos/Video optimizer.app";
const DMG: &str = "target/release/bundle/dmg/Video optimizer_0.1.0_test.dmg";

/// The stub tools. Each logs its call to `$STUB_LOG`; what they print is set
/// by the `STUB_*` variables of the test (the defaults are a good build).
const CODESIGN: &str = r#"#!/bin/bash
echo "codesign $*" >> "$STUB_LOG"
case "$1" in
  --verify)
    if [ -n "${STUB_VERIFY_FAIL:-}" ]; then echo "stub: invalid signature" >&2; exit 1; fi ;;
  -dv)
    echo "Executable=stub" >&2
    echo "${STUB_SIGNATURE:-Signature=adhoc}" >&2
    echo "${STUB_SEAL:-Sealed Resources version=2 rules=13 files=3}" >&2 ;;
esac
"#;

const LIPO: &str = r#"#!/bin/bash
echo "lipo $*" >> "$STUB_LOG"
program="${@: -1}"
case "$program" in
  */ffprobe) echo "${STUB_ARCH_FFPROBE:-${STUB_ARCH:-arm64}}" ;;
  *) echo "${STUB_ARCH:-arm64}" ;;
esac
"#;

const VTOOL: &str = r#"#!/bin/bash
echo "vtool $*" >> "$STUB_LOG"
program="${@: -1}"
minos="${STUB_MINOS:-12.0}"
case "$program" in
  */ffprobe) minos="${STUB_MINOS_FFPROBE:-$minos}" ;;
esac
echo "Load command 9"
echo "      cmd LC_BUILD_VERSION"
echo "  cmdsize 32"
echo " platform MACOS"
if [ "$minos" != none ]; then echo "    minos $minos"; fi
echo "      sdk 14.0"
"#;

const PLUTIL: &str = r#"#!/bin/bash
echo "plutil $*" >> "$STUB_LOG"
if [ "$1" = "-extract" ] && [ -n "${STUB_SEAL_MISSING:-}" ] && [ "$2" = "files2.MacOS/$STUB_SEAL_MISSING" ]; then
  exit 1
fi
if [ "$1" = "-p" ]; then echo "stub seal entries"; fi
exit 0
"#;

const HDIUTIL: &str = r#"#!/bin/bash
echo "hdiutil $*" >> "$STUB_LOG"
case "$1" in
  attach)
    mount=""
    while [ $# -gt 0 ]; do
      if [ "$1" = "-mountpoint" ]; then mount="$2"; fi
      shift
    done
    mkdir -p "$mount/Video optimizer.app"
    cp -R "$STUB_APP/." "$mount/Video optimizer.app"
    if [ -n "${STUB_DMG_BREAK:-}" ]; then
      chmod -x "$mount/Video optimizer.app/Contents/MacOS/ffprobe"
    fi ;;
  detach) rm -rf "$2" ;;
esac
"#;

/// A work folder with a built app and a disk image, and the stub tools.
struct Build {
    dir: TempDir,
}

impl Build {
    fn new() -> Build {
        let build = Build {
            dir: TempDir::new().unwrap(),
        };
        let bin = build.dir.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        for (name, body) in [
            ("codesign", CODESIGN),
            ("lipo", LIPO),
            ("vtool", VTOOL),
            ("plutil", PLUTIL),
            ("hdiutil", HDIUTIL),
        ] {
            write_executable(&bin.join(name), body);
        }
        let macos = build.app().join("Contents/MacOS");
        fs::create_dir_all(&macos).unwrap();
        write_executable(&macos.join("video-optimizer-app"), "app");
        write_executable(&macos.join("ffmpeg"), "ffmpeg");
        write_executable(&macos.join("ffprobe"), "ffprobe");
        let dmg = build.work().join(DMG);
        fs::create_dir_all(dmg.parent().unwrap()).unwrap();
        fs::write(&dmg, "dmg").unwrap();
        build
    }

    /// The folder the script runs in (the repository root on the runner).
    fn work(&self) -> PathBuf {
        self.dir.path().join("work")
    }

    fn app(&self) -> PathBuf {
        self.work().join(APP)
    }

    fn log(&self) -> String {
        fs::read_to_string(self.dir.path().join("calls.log")).unwrap_or_default()
    }

    /// Runs the script with `args`, the stubs first on the path and the given
    /// `STUB_*` settings.
    fn run(&self, args: &[&str], settings: &[(&str, &str)]) -> Output {
        let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/check-mac-app.sh");
        let path = format!("{}:/usr/bin:/bin", self.dir.path().join("bin").display());
        let mut command = Command::new("bash");
        command
            .arg(script)
            .args(args)
            .current_dir(self.work())
            .env("PATH", path)
            .env("STUB_LOG", self.dir.path().join("calls.log"))
            .env("STUB_APP", self.app());
        for (name, value) in settings {
            command.env(name, value);
        }
        command.output().unwrap()
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The script failed, and its message holds every one of `needles`.
fn assert_fails_naming(output: &Output, needles: &[&str]) {
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        stderr(output)
    );
    assert!(
        !output.status.success(),
        "the script passed a broken build: {text}"
    );
    for needle in needles {
        assert!(text.contains(needle), "no {needle:?} in the output: {text}");
    }
}

/// The script passed, and it really looked: each program was asked for its
/// architecture and its build, in the app and in the mounted copy, and the
/// disk image was detached again.
fn assert_passes_and_looked(build: &Build, output: &Output) {
    assert!(output.status.success(), "{}", stderr(output));
    let log = build.log();
    for tool in ["ffmpeg", "ffprobe"] {
        let lipo = log
            .lines()
            .filter(|line| line.starts_with("lipo -archs ") && line.ends_with(&format!("/{tool}")))
            .count();
        let vtool = log
            .lines()
            .filter(|line| {
                line.starts_with("vtool -show-build ") && line.ends_with(&format!("/{tool}"))
            })
            .count();
        assert_eq!(
            (lipo, vtool),
            (2, 2),
            "{tool} in the app and the mounted copy: {log}"
        );
    }
    assert!(log.contains("hdiutil attach"), "{log}");
    assert!(log.contains("hdiutil detach"), "{log}");
}

// ---- builds the script accepts ----

#[test]
fn it_passes_a_good_apple_silicon_app() {
    let build = Build::new();
    let output = build.run(&["arm64"], &[]);
    assert_passes_and_looked(&build, &output);
}

#[test]
fn it_passes_a_good_intel_app() {
    let build = Build::new();
    let output = build.run(&["x86_64"], &[("STUB_ARCH", "x86_64")]);
    assert_passes_and_looked(&build, &output);
}

#[test]
fn it_passes_a_program_that_needs_macos_12_or_older() {
    for minos in ["12.0", "12.0.0", "11.0", "10.15", "12"] {
        let build = Build::new();
        let output = build.run(&["arm64"], &[("STUB_MINOS", minos)]);
        assert_passes_and_looked(&build, &output);
    }
}

// ---- builds it refuses ----

#[test]
fn it_refuses_a_program_that_needs_a_newer_macos() {
    for minos in ["13.0", "12.0.1", "12.1", "12.3", "14"] {
        let build = Build::new();
        let output = build.run(&["arm64"], &[("STUB_MINOS", minos)]);
        assert_fails_naming(&output, &["ffmpeg", &format!("minos {minos}"), "12.0"]);
    }
    // Only ffprobe is too new: it is the one named.
    let build = Build::new();
    let output = build.run(&["arm64"], &[("STUB_MINOS_FFPROBE", "13.0")]);
    assert_fails_naming(&output, &["ffprobe", "minos 13.0"]);
}

#[test]
fn it_refuses_a_program_without_a_minos_line() {
    let build = Build::new();
    let output = build.run(&["arm64"], &[("STUB_MINOS", "none")]);
    assert_fails_naming(&output, &["ffmpeg", "no minos"]);
}

#[test]
fn it_refuses_a_program_of_another_architecture() {
    let build = Build::new();
    let output = build.run(&["arm64"], &[("STUB_ARCH", "x86_64")]);
    assert_fails_naming(&output, &["ffmpeg", "x86_64", "arm64"]);
    // The Intel job given an arm64 program, and only ffprobe wrong.
    let build = Build::new();
    let output = build.run(&["x86_64"], &[("STUB_ARCH", "arm64")]);
    assert_fails_naming(&output, &["ffmpeg", "arm64", "x86_64"]);
    let build = Build::new();
    let output = build.run(&["arm64"], &[("STUB_ARCH_FFPROBE", "x86_64")]);
    assert_fails_naming(&output, &["ffprobe", "x86_64"]);
    // A universal program is not the architecture of the job either.
    let build = Build::new();
    let output = build.run(&["arm64"], &[("STUB_ARCH", "x86_64 arm64")]);
    assert_fails_naming(&output, &["ffmpeg", "x86_64 arm64"]);
}

#[test]
fn it_refuses_a_signature_that_is_not_adhoc() {
    let build = Build::new();
    let output = build.run(&["arm64"], &[("STUB_SIGNATURE", "Signature size=9012")]);
    assert_fails_naming(&output, &["Signature=adhoc"]);
}

#[test]
fn it_refuses_a_seal_without_sealed_resources() {
    let build = Build::new();
    let output = build.run(&["arm64"], &[("STUB_SEAL", "Sealed Resources=none")]);
    assert_fails_naming(&output, &["Sealed Resources version=2"]);
}

#[test]
fn it_refuses_a_signature_that_does_not_verify() {
    let build = Build::new();
    let output = build.run(&["arm64"], &[("STUB_VERIFY_FAIL", "1")]);
    assert_fails_naming(&output, &["invalid signature"]);
}

#[test]
fn it_refuses_a_program_the_seal_does_not_list() {
    for tool in ["ffmpeg", "ffprobe"] {
        let build = Build::new();
        let output = build.run(&["arm64"], &[("STUB_SEAL_MISSING", tool)]);
        assert_fails_naming(&output, &[&format!("does not list MacOS/{tool}")]);
    }
}

#[test]
fn it_refuses_a_program_that_cannot_run_or_is_missing() {
    for tool in ["ffmpeg", "ffprobe"] {
        let build = Build::new();
        let program = build.app().join("Contents/MacOS").join(tool);
        fs::set_permissions(&program, fs::Permissions::from_mode(0o644)).unwrap();
        let output = build.run(&["arm64"], &[]);
        assert_fails_naming(&output, &[&format!("Contents/MacOS/{tool}"), "executable"]);
        let build = Build::new();
        fs::remove_file(build.app().join("Contents/MacOS").join(tool)).unwrap();
        let output = build.run(&["arm64"], &[]);
        assert_fails_naming(&output, &[&format!("Contents/MacOS/{tool}"), "executable"]);
    }
}

#[test]
fn it_refuses_an_app_that_is_only_broken_inside_the_disk_image() {
    let build = Build::new();
    let output = build.run(&["arm64"], &[("STUB_DMG_BREAK", "1")]);
    // The copy in the mounted image is the one named, not the app in bundle/macos.
    assert_fails_naming(&output, &["Video optimizer.app/Contents/MacOS/ffprobe"]);
    assert!(
        !stderr(&output).contains("target/release/bundle/macos"),
        "{}",
        stderr(&output)
    );
    // The image is detached again even though the check failed.
    assert!(build.log().contains("hdiutil detach"), "{}", build.log());
}

#[test]
fn it_refuses_a_run_without_a_disk_image_or_an_architecture() {
    let build = Build::new();
    fs::remove_file(build.work().join(DMG)).unwrap();
    let output = build.run(&["arm64"], &[]);
    assert_fails_naming(&output, &["no .dmg"]);
    assert!(!build.log().contains("hdiutil attach"), "{}", build.log());
    let build = Build::new();
    let output = build.run(&[], &[]);
    assert_fails_naming(&output, &["usage"]);
}
