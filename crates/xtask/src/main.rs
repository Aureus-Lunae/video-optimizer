//! Build helper for the bundle. `cargo tauri build --config
//! src-tauri/tauri.bundle.conf.json` runs `cargo run --locked -p
//! video-optimizer-xtask -- ffmpeg` first (its `beforeBuildCommand`). That
//! reads `ffmpeg/builds.toml`, downloads the pinned archives for the target
//! triple into `ffmpeg-cache/`, checks each SHA-256, takes the two programs
//! out and copies them to `src-tauri/binaries/ffmpeg-<triple>[.exe]` and
//! `ffprobe-<triple>[.exe]`, the names Tauri's `externalBin` looks for. Tauri
//! then installs them beside the app as `ffmpeg[.exe]` and `ffprobe[.exe]`.
//!
//! With `--dest DIR` the programs go to `DIR` as plain `ffmpeg[.exe]` and
//! `ffprobe[.exe]` instead, for `VIDEO_OPTIMIZER_TOOLS_DIR` in CI tests.
//!
//! Downloads run `curl` and extraction runs `tar` (or `unzip`) as child
//! processes with separate arguments, never through a shell. Nothing is
//! staged unless every archive downloaded, matched its checksum and held its
//! program.

use std::env;
use std::ffi::OsString;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};

use sha2::{Digest, Sha256};

/// The variable tauri-cli sets for its `beforeBuildCommand`.
const TRIPLE_VAR: &str = "TAURI_ENV_TARGET_TRIPLE";

/// The triple this helper was built for (see `build.rs`): used when
/// `TAURI_ENV_TARGET_TRIPLE` is not set.
const HOST_TRIPLE: &str = env!("VIDEO_OPTIMIZER_HOST_TRIPLE");

/// The two programs every target stages, in the order they are fetched.
const PROGRAMS: [&str; 2] = ["ffmpeg", "ffprobe"];

/// One archive of `builds.toml`: it gives one program.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Archive {
    url: String,
    /// Lower-case hex SHA-256 of the archive file.
    sha256: String,
    /// `ffmpeg` or `ffprobe`.
    program: String,
    /// The program's path inside the archive (the program's name when the
    /// file does not say).
    inner: String,
}

/// The archives for one target triple.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Target {
    triple: String,
    archives: Vec<Archive>,
}

/// The parsed, checked `builds.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Builds {
    version: String,
    targets: Vec<Target>,
}

/// Where the programs end up.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Destination {
    /// `<dir>/ffmpeg-<triple>[.exe]`: Tauri's `externalBin` names.
    Sidecars(PathBuf),
    /// `<dir>/ffmpeg[.exe]`: for `VIDEO_OPTIMIZER_TOOLS_DIR`.
    Plain(PathBuf),
}

impl Destination {
    fn dir(&self) -> &Path {
        match self {
            Destination::Sidecars(dir) | Destination::Plain(dir) => dir,
        }
    }

    /// The file name `program` gets for `triple`.
    fn file_name(&self, program: &str, triple: &str) -> Result<String, String> {
        match self {
            Destination::Sidecars(_) => sidecar_name(program, Some(triple)),
            Destination::Plain(_) => {
                let triple = check_triple(triple)?;
                Ok(format!("{program}{}", exe_suffix(triple)))
            }
        }
    }
}

/// `.exe` for a Windows triple.
fn exe_suffix(triple: &str) -> &'static str {
    if triple.contains("-windows-") {
        ".exe"
    } else {
        ""
    }
}

/// The triple, trimmed, if it is a plain triple: it becomes part of a path,
/// so an empty one, one with a separator, a space or `..`, and one with no
/// `-` in it are refused.
fn check_triple(triple: &str) -> Result<&str, String> {
    let triple = triple.trim();
    if triple.is_empty() {
        return Err(format!("{TRIPLE_VAR} is empty"));
    }
    let plain = triple
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !plain || triple.contains("..") || !triple.contains('-') {
        return Err(format!("{TRIPLE_VAR} is not a target triple: {triple:?}"));
    }
    Ok(triple)
}

/// `<program>-<triple>`, with `.exe` for a Windows triple: the file name
/// Tauri expects under `src-tauri/binaries/`. A missing triple is refused as
/// well as one [`check_triple`] refuses.
fn sidecar_name(program: &str, triple: Option<&str>) -> Result<String, String> {
    let Some(triple) = triple else {
        return Err(format!(
            "{TRIPLE_VAR} is not set: run this through `cargo tauri build --config src-tauri/tauri.bundle.conf.json`"
        ));
    };
    let triple = check_triple(triple)?;
    Ok(format!("{program}-{triple}{}", exe_suffix(triple)))
}

/// The triple to stage for: the environment's when it is set, else the one
/// this helper was built for.
fn resolve_triple(from_env: Option<&str>, host: &str) -> Result<String, String> {
    check_triple(from_env.unwrap_or(host)).map(str::to_owned)
}

/// `ffmpeg` alone, or `ffmpeg --dest DIR`. Returns the folder, if given.
fn parse_args(args: &[String]) -> Result<Option<PathBuf>, String> {
    match args {
        [command] if command == "ffmpeg" => Ok(None),
        [command, flag, dir] if command == "ffmpeg" && flag == "--dest" => {
            if dir.is_empty() {
                Err("--dest needs a folder".to_owned())
            } else {
                Ok(Some(PathBuf::from(dir)))
            }
        }
        _ => Err("expected `ffmpeg [--dest DIR]`".to_owned()),
    }
}

// --- builds.toml --------------------------------------------------------------

/// A SHA-256 as the vendors publish it: 64 lower-case hex digits.
fn check_sha256(sha256: &str) -> Result<(), String> {
    if sha256.len() == 64
        && sha256
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err(format!(
            "sha256 must be 64 lower-case hex digits, found {sha256:?}"
        ))
    }
}

/// The last part of an https URL, used in the cache file's name: letters,
/// digits, `.`, `_` and `-` only.
fn url_file_name(url: &str) -> &str {
    url.rsplit('/').next().unwrap_or_default()
}

/// A pinned download: https, no whitespace, not a "latest" redirect (which
/// moves), naming the version, ending in a plain file name.
fn check_url(url: &str, version: &str) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err(format!("url must start with https://, found {url:?}"));
    }
    if url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("url has a space or control character: {url:?}"));
    }
    if url.contains("/redirect/latest/") {
        return Err(format!(
            "url is a 'latest' redirect, not a pinned release: {url}"
        ));
    }
    if !url.contains(version) {
        return Err(format!("url must name version {version}: {url}"));
    }
    let name = url_file_name(url);
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(format!("url must end in a plain file name: {url}"));
    }
    Ok(())
}

/// A path inside an archive that cannot leave the folder it is extracted
/// into: relative, `/`-separated, no `.` or `..` part, plain characters, and
/// not starting with `-` (it is a command line argument).
fn check_inner(inner: &str) -> Result<(), String> {
    let plain = inner
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/'));
    let parts_ok = inner
        .split('/')
        .all(|part| !part.is_empty() && part != "." && part != "..");
    if inner.is_empty() || inner.starts_with('-') || !plain || !parts_ok {
        return Err(format!(
            "inner must be a relative path of plain characters inside the archive, found {inner:?}"
        ));
    }
    Ok(())
}

fn text<'a>(table: &'a toml::Table, key: &str, what: &str) -> Result<&'a str, String> {
    match table.get(key) {
        Some(toml::Value::String(value)) => Ok(value),
        Some(other) => Err(format!("{what}: {key} must be a string, found {other}")),
        None => Err(format!("{what}: {key} is missing")),
    }
}

/// Reads and checks `builds.toml`: a version; at least one target; per
/// target a plain, unique triple and exactly one `ffmpeg` and one `ffprobe`
/// archive; per archive a pinned https URL naming the version, a 64-digit
/// lower-case SHA-256, a program and a safe `inner` path. One URL may not
/// appear with two checksums.
fn parse_builds(text_of_file: &str) -> Result<Builds, String> {
    let table: toml::Table = text_of_file
        .parse()
        .map_err(|error| format!("builds.toml is not TOML: {error}"))?;
    let version = text(&table, "version", "builds.toml")?.trim().to_owned();
    if version.is_empty() {
        return Err("builds.toml: version is empty".to_owned());
    }
    let Some(entries) = table.get("target").and_then(toml::Value::as_array) else {
        return Err("builds.toml: no [[target]] entries".to_owned());
    };
    let mut targets: Vec<Target> = Vec::new();
    for (number, entry) in entries.iter().enumerate() {
        let what = format!("target {}", number + 1);
        let Some(entry) = entry.as_table() else {
            return Err(format!("{what} is not a table"));
        };
        let triple = text(entry, "triple", &what)?;
        let triple = check_triple(triple)
            .map_err(|error| format!("{what}: triple: {error}"))?
            .to_owned();
        if targets.iter().any(|target| target.triple == triple) {
            return Err(format!("triple {triple} is listed more than once"));
        }
        let what = format!("target {triple}");
        let Some(list) = entry.get("archives").and_then(toml::Value::as_array) else {
            return Err(format!("{what}: archives is missing"));
        };
        let mut archives = Vec::new();
        for item in list {
            let Some(item) = item.as_table() else {
                return Err(format!("{what}: an archive is not a table"));
            };
            let url = text(item, "url", &what)?;
            check_url(url, &version).map_err(|error| format!("{what}: {error}"))?;
            let sha256 = text(item, "sha256", &what)?;
            check_sha256(sha256).map_err(|error| format!("{what}: {error}"))?;
            let program = text(item, "program", &what)?;
            if !PROGRAMS.contains(&program) {
                return Err(format!(
                    "{what}: program must be one of {PROGRAMS:?}, found {program:?}"
                ));
            }
            let inner = match item.get("inner") {
                None => program,
                Some(_) => text(item, "inner", &what)?,
            };
            check_inner(inner).map_err(|error| format!("{what}: {error}"))?;
            archives.push(Archive {
                url: url.to_owned(),
                sha256: sha256.to_owned(),
                program: program.to_owned(),
                inner: inner.to_owned(),
            });
        }
        let each_once = PROGRAMS
            .iter()
            .all(|program| archives.iter().filter(|a| a.program == *program).count() == 1);
        if archives.len() != PROGRAMS.len() || !each_once {
            return Err(format!(
                "{what}: archives must hold exactly one ffmpeg and one ffprobe"
            ));
        }
        targets.push(Target { triple, archives });
    }
    if targets.is_empty() {
        return Err("builds.toml: no [[target]] entries".to_owned());
    }
    let archives: Vec<&Archive> = targets.iter().flat_map(|t| &t.archives).collect();
    for archive in &archives {
        if archives
            .iter()
            .any(|other| other.url == archive.url && other.sha256 != archive.sha256)
        {
            return Err(format!(
                "{} is listed twice with different sha256 values",
                archive.url
            ));
        }
    }
    Ok(Builds { version, targets })
}

// --- downloading --------------------------------------------------------------

/// The SHA-256 of a file, as lower-case hex.
fn sha256_of(path: &Path) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// The arguments `curl` gets to download `url` to `file`: fail on an HTTP
/// error, follow redirects, print only errors, give up on a connection after
/// 30 seconds and on the whole download after 15 minutes, and try a
/// transient failure twice more. An `https://` URL may not be redirected to
/// any other protocol (`--proto =https --proto-redir =https`); the tests'
/// `file://` URLs get the same limits without that rule. The URL comes last.
fn curl_args(url: &str, file: &Path) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "--fail",
        "--location",
        "--silent",
        "--show-error",
        "--connect-timeout",
        "30",
        "--max-time",
        "900",
        "--retry",
        "2",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    if url.starts_with("https://") {
        for arg in ["--proto", "=https", "--proto-redir", "=https"] {
            args.push(arg.into());
        }
    }
    args.push("--output".into());
    args.push(file.into());
    args.push(url.into());
    args
}

/// Downloads `url` to `file` with `curl` and the arguments of [`curl_args`],
/// a child process with separate arguments and no shell. A URL that starts
/// with `-` would be read as an option and is refused.
fn fetch_with_curl(url: &str, file: &Path) -> Result<(), String> {
    if url.starts_with('-') {
        return Err(format!("refusing a URL that starts with '-': {url}"));
    }
    let output = Command::new("curl")
        .args(curl_args(url, file))
        .stdin(Stdio::null())
        .output()
        .map_err(|error| format!("cannot start curl: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "curl failed for {url} ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Where an archive is cached: `<cache>/<first 12 digits of its SHA-256>-<file
/// name from the URL>`. The checksum in the name keeps two archives that
/// share a file name (`ffmpeg.zip` for arm64 and for amd64) apart, and one
/// archive that two entries name is one file.
fn cache_file(cache: &Path, url: &str, sha256: &str) -> PathBuf {
    let short = sha256.get(..12).unwrap_or(sha256);
    cache.join(format!("{short}-{}", url_file_name(url)))
}

/// Makes `file` the archive with the given SHA-256. A cached file that
/// already matches is kept (`Ok(false)`: nothing was downloaded). Otherwise
/// the old file is removed, `fetch` downloads `url` to `file`, and the
/// result is checked: a failed fetch or a checksum mismatch deletes the file
/// and is an error (`Ok(true)` means it was downloaded now).
fn download(
    url: &str,
    sha256: &str,
    file: &Path,
    fetch: &mut dyn FnMut(&str, &Path) -> Result<(), String>,
) -> Result<bool, String> {
    if file.is_file() && sha256_of(file).is_ok_and(|found| found == sha256) {
        return Ok(false);
    }
    if let Some(folder) = file.parent() {
        fs::create_dir_all(folder)
            .map_err(|error| format!("cannot create {}: {error}", folder.display()))?;
    }
    let _ = fs::remove_file(file);
    if let Err(error) = fetch(url, file) {
        let _ = fs::remove_file(file);
        return Err(error);
    }
    match sha256_of(file) {
        Ok(found) if found == sha256 => Ok(true),
        Ok(found) => {
            let _ = fs::remove_file(file);
            Err(format!(
                "checksum mismatch for {url}: expected {sha256}, got {found}; the download was deleted"
            ))
        }
        Err(error) => {
            let _ = fs::remove_file(file);
            Err(error)
        }
    }
}

// --- extracting ---------------------------------------------------------------

/// Runs each `(program, arguments)` in turn until one exits successfully.
/// When none does, the error names every attempt and why it failed.
fn run_until_success(attempts: &[(String, Vec<OsString>)]) -> Result<(), String> {
    let mut failures = Vec::new();
    for (program, args) in attempts {
        let shown = args
            .iter()
            .map(|arg| arg.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");
        match Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .output()
        {
            Ok(output) if output.status.success() => return Ok(()),
            Ok(output) => failures.push(format!(
                "`{program} {shown}` failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )),
            Err(error) => failures.push(format!("cannot start `{program} {shown}`: {error}")),
        }
    }
    if failures.is_empty() {
        failures.push("no program to run".to_owned());
    }
    Err(failures.join("; "))
}

/// Takes `member` out of `archive` into a fresh `into` folder and returns
/// the extracted file's path (`into/<member>`). `tar -xf` reads `.zip` on
/// macOS and Windows 10 and later; GNU tar (Linux) does not, so `unzip` is
/// the second try. An archive that does not hold the member is an error.
fn extract(archive: &Path, member: &str, into: &Path) -> Result<PathBuf, String> {
    let _ = fs::remove_dir_all(into);
    fs::create_dir_all(into)
        .map_err(|error| format!("cannot create {}: {error}", into.display()))?;
    let os = |text: &str| OsString::from(text);
    let attempts = [
        (
            "tar".to_owned(),
            vec![os("-xf"), archive.into(), os("-C"), into.into(), os(member)],
        ),
        (
            "unzip".to_owned(),
            vec![
                os("-q"),
                os("-o"),
                archive.into(),
                os(member),
                os("-d"),
                into.into(),
            ],
        ),
    ];
    run_until_success(&attempts).map_err(|error| {
        format!(
            "cannot extract {member} from {}: {error}",
            archive.display()
        )
    })?;
    let program = into.join(member);
    if program.is_file() {
        Ok(program)
    } else {
        Err(format!(
            "cannot extract {member} from {}: the archive has no such file",
            archive.display()
        ))
    }
}

// --- staging ------------------------------------------------------------------

/// Downloads, checks, extracts and copies the programs of `triple`. Works in
/// three passes, so that a failure leaves nothing in the destination: every
/// archive is downloaded and checked, then every program is extracted, and
/// only then are the programs copied (mode 0755 where that means anything).
/// A triple with no entry is an error that says to use the system ffmpeg.
/// Returns the staged files, `ffmpeg` first.
fn stage(
    builds: &Builds,
    triple: &str,
    cache: &Path,
    dest: &Destination,
    fetch: &mut dyn FnMut(&str, &Path) -> Result<(), String>,
) -> Result<Vec<PathBuf>, String> {
    let Some(target) = builds.targets.iter().find(|target| target.triple == triple) else {
        return Err(format!(
            "no ffmpeg build is pinned for {triple}: use the system ffmpeg instead \
             (install ffmpeg and ffprobe, or point VIDEO_OPTIMIZER_TOOLS_DIR at a folder holding them)"
        ));
    };
    let names = target
        .archives
        .iter()
        .map(|archive| dest.file_name(&archive.program, triple))
        .collect::<Result<Vec<_>, _>>()?;

    let mut files = Vec::new();
    for archive in &target.archives {
        let file = cache_file(cache, &archive.url, &archive.sha256);
        download(&archive.url, &archive.sha256, &file, fetch)?;
        files.push(file);
    }

    let extract_root = cache.join("extract");
    let mut programs = Vec::new();
    for (archive, file) in target.archives.iter().zip(&files) {
        let into = extract_root.join(format!("{triple}-{}", archive.program));
        programs.push(extract(file, &archive.inner, &into)?);
    }

    fs::create_dir_all(dest.dir())
        .map_err(|error| format!("cannot create {}: {error}", dest.dir().display()))?;
    let mut staged = Vec::new();
    for (program, name) in programs.iter().zip(names) {
        let target_file = dest.dir().join(name);
        fs::copy(program, &target_file).map_err(|error| {
            format!(
                "cannot copy {} to {}: {error}",
                program.display(),
                target_file.display()
            )
        })?;
        make_executable(&target_file)?;
        staged.push(target_file);
    }
    // The extracted copies are only needed until here.
    let _ = fs::remove_dir_all(&extract_root);
    Ok(staged)
}

#[cfg(unix)]
fn make_executable(file: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(file, fs::Permissions::from_mode(0o755))
        .map_err(|error| format!("cannot make {} executable: {error}", file.display()))
}

#[cfg(not(unix))]
fn make_executable(_file: &Path) -> Result<(), String> {
    Ok(())
}

/// The whole job: the triple, `builds.toml`, then [`stage`] with `curl`.
fn run(
    root: &Path,
    dest: Option<PathBuf>,
    triple_from_env: Option<&str>,
) -> Result<Vec<PathBuf>, String> {
    let triple = resolve_triple(triple_from_env, HOST_TRIPLE)?;
    let builds_file = root.join("ffmpeg").join("builds.toml");
    let text = fs::read_to_string(&builds_file)
        .map_err(|error| format!("cannot read {}: {error}", builds_file.display()))?;
    let builds = parse_builds(&text)?;
    let dest = match dest {
        Some(dir) => Destination::Plain(dir),
        None => Destination::Sidecars(root.join("src-tauri").join("binaries")),
    };
    stage(
        &builds,
        &triple,
        &root.join("ffmpeg-cache"),
        &dest,
        &mut |url, file| {
            eprintln!("video-optimizer-xtask: downloading {url}");
            fetch_with_curl(url, file)
        },
    )
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let dest = match parse_args(&args) {
        Ok(dest) => dest,
        Err(error) => {
            eprintln!("video-optimizer-xtask: {error}");
            eprintln!("usage: video-optimizer-xtask ffmpeg [--dest DIR]");
            return ExitCode::from(2);
        }
    };
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let triple = env::var(TRIPLE_VAR).ok();
    match run(&root, dest, triple.as_deref()) {
        Ok(staged) => {
            for path in staged {
                eprintln!("video-optimizer-xtask: staged {}", path.display());
            }
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("video-optimizer-xtask: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::*;

    const MAC_ARM: &str = "aarch64-apple-darwin";
    const MAC_INTEL: &str = "x86_64-apple-darwin";
    const WINDOWS: &str = "x86_64-pc-windows-msvc";
    const LINUX: &str = "x86_64-unknown-linux-gnu";

    fn hash(bytes: &[u8]) -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn file_url(path: &Path) -> String {
        let path = path.to_string_lossy().replace('\\', "/");
        if path.starts_with('/') {
            format!("file://{path}")
        } else {
            format!("file:///{path}")
        }
    }

    /// A `.tar` in `folder` holding the given `(member path, bytes)` files.
    /// `tar -xf` reads it on every platform the build runs on.
    fn make_tar(folder: &Path, name: &str, members: &[(&str, &[u8])]) -> PathBuf {
        let source = folder.join(format!("{name}-source"));
        for (member, bytes) in members {
            let path = source.join(member);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let archive = folder.join(name);
        let mut tar = Command::new("tar");
        tar.arg("-cf").arg(&archive).arg("-C").arg(&source);
        for (member, _) in members {
            tar.arg(member);
        }
        assert!(tar.status().unwrap().success(), "tar -cf made {name}");
        archive
    }

    fn archive(url: &str, sha256: &str, program: &str, inner: &str) -> Archive {
        Archive {
            url: url.to_owned(),
            sha256: sha256.to_owned(),
            program: program.to_owned(),
            inner: inner.to_owned(),
        }
    }

    fn builds_with(triple: &str, archives: Vec<Archive>) -> Builds {
        Builds {
            version: "9.1".to_owned(),
            targets: vec![Target {
                triple: triple.to_owned(),
                archives,
            }],
        }
    }

    /// A fetch that copies the local file a `fake://<name>` URL names from
    /// `served`, and counts its calls.
    fn serving<'a>(
        served: &'a Path,
        calls: &'a Cell<usize>,
    ) -> impl FnMut(&str, &Path) -> Result<(), String> + 'a {
        move |url, file| {
            calls.set(calls.get() + 1);
            let name = url.strip_prefix("fake://").expect("a fake URL");
            fs::copy(served.join(name), file)
                .map(|_| ())
                .map_err(|error| error.to_string())
        }
    }

    // --- the sidecar's name -------------------------------------------------------

    #[test]
    fn sidecar_names_for_each_triple() {
        assert_eq!(
            sidecar_name("ffmpeg", Some(MAC_ARM)).unwrap(),
            "ffmpeg-aarch64-apple-darwin"
        );
        assert_eq!(
            sidecar_name("ffmpeg", Some(MAC_INTEL)).unwrap(),
            "ffmpeg-x86_64-apple-darwin"
        );
        assert_eq!(
            sidecar_name("ffprobe", Some(WINDOWS)).unwrap(),
            "ffprobe-x86_64-pc-windows-msvc.exe"
        );
        // Only a Windows triple gets the suffix.
        assert!(
            !sidecar_name("ffprobe", Some(MAC_ARM))
                .unwrap()
                .ends_with(".exe")
        );
    }

    #[test]
    fn rejects_an_empty_or_non_triple() {
        for triple in [
            None,
            Some(""),
            Some("   "),
            Some("../x"),
            Some("abc"),
            Some("x86_64/apple-darwin"),
            Some("x86_64\\pc-windows-msvc"),
            Some("x86_64 -apple-darwin"),
            Some("x86_64..apple-darwin"),
        ] {
            assert!(sidecar_name("ffmpeg", triple).is_err(), "{triple:?}");
        }
        let error = sidecar_name("ffmpeg", None).unwrap_err();
        assert!(error.contains("TAURI_ENV_TARGET_TRIPLE"), "{error}");
    }

    #[test]
    fn the_triple_is_the_environments_or_else_the_hosts() {
        assert_eq!(resolve_triple(None, LINUX), Ok(LINUX.to_owned()));
        assert_eq!(resolve_triple(Some(WINDOWS), LINUX), Ok(WINDOWS.to_owned()));
        for bad in ["", "  ", "../x", "abc"] {
            assert!(resolve_triple(Some(bad), LINUX).is_err(), "{bad:?}");
        }
    }

    // --- the command line ---------------------------------------------------------

    #[test]
    fn the_ffmpeg_command_takes_an_optional_dest() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(parse_args(&args(&["ffmpeg"])), Ok(None));
        assert_eq!(
            parse_args(&args(&["ffmpeg", "--dest", "out"])),
            Ok(Some(PathBuf::from("out")))
        );
    }

    #[test]
    fn any_other_command_line_is_refused() {
        let args = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        for bad in [
            args(&[]),
            args(&["sidecar"]),
            args(&["ffmpeg", "--dest"]),
            args(&["ffmpeg", "--dest", ""]),
            args(&["ffmpeg", "--dest", "a", "b"]),
            args(&["ffmpeg", "--to", "a"]),
            args(&["ffmpeg", "extra"]),
        ] {
            assert!(parse_args(&bad).is_err(), "{bad:?}");
        }
    }

    // --- builds.toml --------------------------------------------------------------

    fn real_builds_text() -> String {
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ffmpeg/builds.toml"))
            .expect("ffmpeg/builds.toml exists")
    }

    #[test]
    fn builds_toml_has_all_three_triples_with_64_hex_sha256() {
        let builds = parse_builds(&real_builds_text()).expect("builds.toml passes the checker");
        assert_eq!(builds.version, "9.1");
        let triples: Vec<&str> = builds.targets.iter().map(|t| t.triple.as_str()).collect();
        assert_eq!(triples, [MAC_ARM, MAC_INTEL, WINDOWS]);
        for target in &builds.targets {
            let programs: Vec<&str> = target.archives.iter().map(|a| a.program.as_str()).collect();
            assert_eq!(programs, ["ffmpeg", "ffprobe"], "{}", target.triple);
            for archive in &target.archives {
                assert_eq!(archive.sha256.len(), 64, "{}", archive.url);
                assert!(
                    archive
                        .sha256
                        .chars()
                        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
                    "{}",
                    archive.url
                );
                assert!(archive.url.starts_with("https://"), "{}", archive.url);
                assert!(archive.url.contains("9.1"), "{}", archive.url);
            }
        }
    }

    /// A builds.toml that passes the checker, for the fixtures to change one
    /// thing in.
    fn good_toml() -> String {
        let sha = "a".repeat(64);
        let other = "b".repeat(64);
        format!(
            r#"
version = "9.1"

[[target]]
triple = "aarch64-apple-darwin"
archives = [
  {{ url = "https://example.test/mac/111_9.1/ffmpeg.zip", sha256 = "{sha}", program = "ffmpeg" }},
  {{ url = "https://example.test/mac/111_9.1/ffprobe.zip", sha256 = "{other}", program = "ffprobe" }},
]

[[target]]
triple = "x86_64-pc-windows-msvc"
archives = [
  {{ url = "https://example.test/win/9.1/all.zip", sha256 = "{sha}", program = "ffmpeg", inner = "top/bin/ffmpeg.exe" }},
  {{ url = "https://example.test/win/9.1/all.zip", sha256 = "{sha}", program = "ffprobe", inner = "top/bin/ffprobe.exe" }},
]
"#
        )
    }

    #[test]
    fn the_checker_accepts_a_good_file_and_defaults_inner_to_the_program() {
        let builds = parse_builds(&good_toml()).expect("good fixture");
        assert_eq!(builds.targets[0].archives[0].inner, "ffmpeg");
        assert_eq!(builds.targets[1].archives[1].inner, "top/bin/ffprobe.exe");
    }

    #[test]
    fn the_checker_refuses_a_63_character_hash_and_an_http_url() {
        let good = good_toml();
        let sha = "a".repeat(64);
        let short = good.replacen(&sha, &"a".repeat(63), 1);
        assert_ne!(short, good, "the fixture changed");
        let error = parse_builds(&short).unwrap_err();
        assert!(error.contains("sha256"), "{error}");

        let http = good.replacen("https://example.test/mac", "http://example.test/mac", 1);
        assert_ne!(http, good);
        let error = parse_builds(&http).unwrap_err();
        assert!(error.contains("https"), "{error}");
    }

    #[test]
    fn the_checker_refuses_other_bad_entries() {
        let good = good_toml();
        let sha = "a".repeat(64);
        let cases: Vec<(String, &str)> = vec![
            // upper case, or not hex
            (good.replacen(&sha, &"A".repeat(64), 1), "sha256"),
            (good.replacen(&sha, &"g".repeat(64), 1), "sha256"),
            (good.replacen(&sha, &"a".repeat(65), 1), "sha256"),
            // not pinned: the moving "latest" redirect
            (
                good.replacen(
                    "/mac/111_9.1/ffmpeg.zip",
                    "/redirect/latest/mac/ffmpeg.zip",
                    1,
                ),
                "latest",
            ),
            // a URL that does not name the version
            (
                good.replacen("/mac/111_9.1/ffmpeg.zip", "/mac/111_9.0/ffmpeg.zip", 1),
                "9.1",
            ),
            (
                good.replacen("https://example.test", "ftp://example.test", 1),
                "https",
            ),
            // a program other than the two
            (
                good.replacen("program = \"ffmpeg\"", "program = \"ffplay\"", 1),
                "program",
            ),
            // an inner path that leaves the folder
            (
                good.replacen("top/bin/ffmpeg.exe", "../ffmpeg.exe", 1),
                "inner",
            ),
            (
                good.replacen("top/bin/ffmpeg.exe", "/etc/passwd", 1),
                "inner",
            ),
            (
                good.replacen("top/bin/ffmpeg.exe", "top\\bin\\ffmpeg.exe", 1),
                "inner",
            ),
            // a triple that is a path
            (good.replacen("aarch64-apple-darwin", "../x", 1), "triple"),
            // the same URL with two different checksums
            (
                good.replacen(&sha, &"c".repeat(64), 2),
                "twice with different sha256",
            ),
            // a target with only one of the two programs
            (
                good.replacen("program = \"ffprobe\"", "program = \"ffmpeg\"", 1),
                "exactly one ffmpeg and one ffprobe",
            ),
            // a missing key, the same triple twice, no version, not TOML
            (good.replacen("version = \"9.1\"", "", 1), "version"),
            (
                good.replacen("x86_64-pc-windows-msvc", "aarch64-apple-darwin", 1),
                "more than once",
            ),
            (
                good.replacen("triple = \"aarch64-apple-darwin\"", "", 1),
                "triple",
            ),
            ("not = [toml".to_owned(), "not TOML"),
        ];
        for (text, needle) in cases {
            let error = parse_builds(&text).unwrap_err();
            assert!(error.contains(needle), "{needle}: {error}");
        }
    }

    // --- checksum and download -------------------------------------------------------

    #[test]
    fn a_wrong_checksum_is_refused_and_the_file_deleted() {
        let folder = TempDir::new().unwrap();
        let served = make_tar(folder.path(), "a.tar", &[("ffmpeg", b"bytes")]);
        let cached = folder.path().join("cache").join("a.tar");
        fs::create_dir_all(cached.parent().unwrap()).unwrap();

        let error = download(
            &file_url(&served),
            &"0".repeat(64),
            &cached,
            &mut fetch_with_curl,
        )
        .unwrap_err();

        assert!(error.contains("checksum"), "{error}");
        assert!(
            error.contains(&"0".repeat(64)),
            "names the expected sum: {error}"
        );
        assert!(!cached.exists(), "the refused download must be deleted");
    }

    #[test]
    fn the_right_checksum_is_accepted() {
        let folder = TempDir::new().unwrap();
        let served = make_tar(folder.path(), "a.tar", &[("ffmpeg", b"bytes")]);
        let sum = hash(&fs::read(&served).unwrap());
        let cached = folder.path().join("a-copy.tar");

        let fetched = download(&file_url(&served), &sum, &cached, &mut fetch_with_curl).unwrap();

        assert!(fetched, "it was downloaded");
        assert_eq!(fs::read(&cached).unwrap(), fs::read(&served).unwrap());
    }

    #[test]
    fn a_download_that_fails_is_an_error_and_leaves_no_file() {
        let folder = TempDir::new().unwrap();
        let missing = folder.path().join("no-such-archive.zip");
        let cached = folder.path().join("cached.zip");

        let error = download(
            &file_url(&missing),
            &"0".repeat(64),
            &cached,
            &mut fetch_with_curl,
        )
        .unwrap_err();

        assert!(error.contains("curl"), "{error}");
        assert!(!cached.exists());
    }

    #[test]
    fn a_failed_fetch_that_left_a_partial_file_has_it_deleted() {
        let folder = TempDir::new().unwrap();
        let cached = folder.path().join("partial.zip");
        let mut fetch = |_: &str, file: &Path| {
            fs::write(file, b"half").unwrap();
            Err("connection reset".to_owned())
        };

        let error = download("fake://x", &"0".repeat(64), &cached, &mut fetch).unwrap_err();

        assert!(error.contains("connection reset"), "{error}");
        assert!(!cached.exists());
    }

    #[test]
    fn a_cached_file_with_the_right_checksum_is_not_downloaded_again() {
        let folder = TempDir::new().unwrap();
        let cached = folder.path().join("a.zip");
        fs::write(&cached, b"archive").unwrap();
        let calls = Cell::new(0);
        let mut fetch = |_: &str, _: &Path| {
            calls.set(calls.get() + 1);
            Ok(())
        };

        let fetched = download("fake://a", &hash(b"archive"), &cached, &mut fetch).unwrap();

        assert!(!fetched);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn a_cached_file_with_another_checksum_is_replaced_by_a_new_download() {
        let folder = TempDir::new().unwrap();
        let served = folder.path().join("served");
        fs::create_dir_all(&served).unwrap();
        fs::write(served.join("a"), b"new archive").unwrap();
        let cached = folder.path().join("a.zip");
        fs::write(&cached, b"old, damaged archive").unwrap();
        let calls = Cell::new(0);

        let fetched = download(
            "fake://a",
            &hash(b"new archive"),
            &cached,
            &mut serving(&served, &calls),
        )
        .unwrap();

        assert!(fetched);
        assert_eq!(calls.get(), 1);
        assert_eq!(fs::read(&cached).unwrap(), b"new archive");
    }

    // --- extracting -------------------------------------------------------------------

    #[test]
    fn extract_takes_one_member_out_of_an_archive() {
        let folder = TempDir::new().unwrap();
        let tar = make_tar(
            folder.path(),
            "a.tar",
            &[
                ("top/bin/ffmpeg.exe", b"the program"),
                ("top/other.txt", b"x"),
            ],
        );
        let into = folder.path().join("out");

        let program = extract(&tar, "top/bin/ffmpeg.exe", &into).unwrap();

        assert_eq!(program, into.join("top/bin/ffmpeg.exe"));
        assert_eq!(fs::read(&program).unwrap(), b"the program");
        assert!(!into.join("top/other.txt").exists(), "only the member");
    }

    #[test]
    fn extract_refuses_a_file_that_is_not_an_archive() {
        let folder = TempDir::new().unwrap();
        let junk = folder.path().join("junk.zip");
        fs::write(&junk, b"not an archive at all").unwrap();

        let error = extract(&junk, "ffmpeg", &folder.path().join("out")).unwrap_err();

        assert!(error.contains("extract"), "{error}");
    }

    #[test]
    fn extract_refuses_an_archive_without_the_member() {
        let folder = TempDir::new().unwrap();
        let tar = make_tar(folder.path(), "a.tar", &[("ffmpeg", b"x")]);

        let error = extract(&tar, "ffprobe", &folder.path().join("out")).unwrap_err();

        assert!(error.contains("ffprobe"), "{error}");
    }

    #[test]
    fn extract_clears_what_an_earlier_run_left_in_the_folder() {
        let folder = TempDir::new().unwrap();
        let tar = make_tar(folder.path(), "a.tar", &[("ffmpeg", b"new")]);
        let into = folder.path().join("out");
        fs::create_dir_all(&into).unwrap();
        fs::write(into.join("ffmpeg"), b"left over from the last run").unwrap();
        fs::write(into.join("stray"), b"x").unwrap();

        let program = extract(&tar, "ffmpeg", &into).unwrap();

        assert_eq!(fs::read(&program).unwrap(), b"new");
        assert!(!into.join("stray").exists());
    }

    /// The test program itself: `--list` exits 0, an unknown flag exits 1.
    fn me() -> String {
        std::env::current_exe()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    }

    fn attempt(program: &str, args: &[&str]) -> (String, Vec<std::ffi::OsString>) {
        (
            program.to_owned(),
            args.iter().map(std::ffi::OsString::from).collect(),
        )
    }

    #[test]
    fn the_next_extractor_runs_when_the_first_one_fails() {
        // A program that does not exist, then one that exits 1, then one that works.
        let ran = run_until_success(&[
            attempt("no-such-program-for-video-optimizer", &[]),
            attempt(&me(), &["--no-such-flag"]),
            attempt(&me(), &["--list"]),
        ]);
        assert_eq!(ran, Ok(()));
    }

    #[test]
    fn the_first_extractor_that_works_ends_the_search() {
        // A failing attempt after a working one must not turn it into an error.
        let ran = run_until_success(&[
            attempt(&me(), &["--list"]),
            attempt(&me(), &["--no-such-flag"]),
        ]);
        assert_eq!(ran, Ok(()));
    }

    #[test]
    fn every_extractor_failing_is_an_error_that_names_each() {
        let error = run_until_success(&[
            attempt("no-such-program-for-video-optimizer", &[]),
            attempt(&me(), &["--no-such-flag"]),
        ])
        .unwrap_err();
        assert!(
            error.contains("no-such-program-for-video-optimizer"),
            "{error}"
        );
        assert!(error.contains("--no-such-flag"), "{error}");
        assert!(run_until_success(&[]).is_err());
    }

    // --- staging -----------------------------------------------------------------------

    /// Two archives, one per program, as Martin Riedl publishes them.
    fn mac_setup(folder: &Path) -> (Builds, PathBuf) {
        let served = folder.join("served");
        fs::create_dir_all(&served).unwrap();
        let ffmpeg = make_tar(&served, "ffmpeg.tar", &[("ffmpeg", b"ffmpeg program")]);
        let ffprobe = make_tar(&served, "ffprobe.tar", &[("ffprobe", b"ffprobe program")]);
        let builds = builds_with(
            MAC_ARM,
            vec![
                archive(
                    "fake://ffmpeg.tar",
                    &hash(&fs::read(ffmpeg).unwrap()),
                    "ffmpeg",
                    "ffmpeg",
                ),
                archive(
                    "fake://ffprobe.tar",
                    &hash(&fs::read(ffprobe).unwrap()),
                    "ffprobe",
                    "ffprobe",
                ),
            ],
        );
        (builds, served)
    }

    /// One archive holding both programs under a version folder, as Gyan does.
    fn windows_setup(folder: &Path) -> (Builds, PathBuf) {
        let served = folder.join("served");
        fs::create_dir_all(&served).unwrap();
        let both = make_tar(
            &served,
            "all.tar",
            &[
                ("top/bin/ffmpeg.exe", b"ffmpeg.exe program"),
                ("top/bin/ffprobe.exe", b"ffprobe.exe program"),
            ],
        );
        let sum = hash(&fs::read(both).unwrap());
        let builds = builds_with(
            WINDOWS,
            vec![
                archive("fake://all.tar", &sum, "ffmpeg", "top/bin/ffmpeg.exe"),
                archive("fake://all.tar", &sum, "ffprobe", "top/bin/ffprobe.exe"),
            ],
        );
        (builds, served)
    }

    fn files_in(folder: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(folder)
            .map(|entries| {
                entries
                    .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn staging_copies_both_programs_under_their_sidecar_names() {
        let folder = TempDir::new().unwrap();
        let (builds, served) = mac_setup(folder.path());
        let binaries = folder.path().join("binaries");
        let calls = Cell::new(0);

        let staged = stage(
            &builds,
            MAC_ARM,
            &folder.path().join("cache"),
            &Destination::Sidecars(binaries.clone()),
            &mut serving(&served, &calls),
        )
        .unwrap();

        assert_eq!(
            staged,
            [
                binaries.join("ffmpeg-aarch64-apple-darwin"),
                binaries.join("ffprobe-aarch64-apple-darwin")
            ]
        );
        assert_eq!(fs::read(&staged[0]).unwrap(), b"ffmpeg program");
        assert_eq!(fs::read(&staged[1]).unwrap(), b"ffprobe program");
        assert_eq!(calls.get(), 2, "one archive per program");
    }

    #[cfg(unix)]
    #[test]
    fn staged_programs_are_executable_whatever_the_archive_said() {
        use std::os::unix::fs::PermissionsExt;
        let folder = TempDir::new().unwrap();
        let (builds, served) = mac_setup(folder.path());
        let binaries = folder.path().join("binaries");
        // Even a program the archive stored as 0644.
        let calls = Cell::new(0);
        let staged = stage(
            &builds,
            MAC_ARM,
            &folder.path().join("cache"),
            &Destination::Sidecars(binaries),
            &mut serving(&served, &calls),
        )
        .unwrap();
        for program in &staged {
            let mode = fs::metadata(program).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o755, "{}", program.display());
        }
    }

    #[test]
    fn one_archive_that_holds_both_programs_is_downloaded_once() {
        let folder = TempDir::new().unwrap();
        let (builds, served) = windows_setup(folder.path());
        let binaries = folder.path().join("binaries");
        let calls = Cell::new(0);

        let staged = stage(
            &builds,
            WINDOWS,
            &folder.path().join("cache"),
            &Destination::Sidecars(binaries.clone()),
            &mut serving(&served, &calls),
        )
        .unwrap();

        assert_eq!(
            staged,
            [
                binaries.join("ffmpeg-x86_64-pc-windows-msvc.exe"),
                binaries.join("ffprobe-x86_64-pc-windows-msvc.exe")
            ]
        );
        assert_eq!(fs::read(&staged[0]).unwrap(), b"ffmpeg.exe program");
        assert_eq!(fs::read(&staged[1]).unwrap(), b"ffprobe.exe program");
        assert_eq!(calls.get(), 1, "the shared archive comes down once");
    }

    #[test]
    fn a_second_run_uses_the_cache() {
        let folder = TempDir::new().unwrap();
        let (builds, served) = mac_setup(folder.path());
        let cache = folder.path().join("cache");
        let calls = Cell::new(0);
        let dest = Destination::Sidecars(folder.path().join("binaries"));

        stage(
            &builds,
            MAC_ARM,
            &cache,
            &dest,
            &mut serving(&served, &calls),
        )
        .unwrap();
        assert_eq!(calls.get(), 2);
        stage(
            &builds,
            MAC_ARM,
            &cache,
            &dest,
            &mut serving(&served, &calls),
        )
        .unwrap();

        assert_eq!(calls.get(), 2, "nothing was downloaded the second time");
    }

    #[test]
    fn dest_gives_plain_names_and_the_exe_suffix_follows_the_triple() {
        let folder = TempDir::new().unwrap();
        let calls = Cell::new(0);

        let (mac, served) = mac_setup(&folder.path().join("mac"));
        let out = folder.path().join("out-mac");
        stage(
            &mac,
            MAC_ARM,
            &folder.path().join("cache-mac"),
            &Destination::Plain(out.clone()),
            &mut serving(&served, &calls),
        )
        .unwrap();
        assert_eq!(files_in(&out), ["ffmpeg", "ffprobe"]);

        let (windows, served) = windows_setup(&folder.path().join("win"));
        let out = folder.path().join("out-win");
        stage(
            &windows,
            WINDOWS,
            &folder.path().join("cache-win"),
            &Destination::Plain(out.clone()),
            &mut serving(&served, &calls),
        )
        .unwrap();
        assert_eq!(files_in(&out), ["ffmpeg.exe", "ffprobe.exe"]);
    }

    #[test]
    fn a_triple_with_no_entry_is_refused_with_the_system_ffmpeg_hint_and_nothing_happens() {
        let folder = TempDir::new().unwrap();
        let (builds, served) = mac_setup(folder.path());
        let calls = Cell::new(0);
        let binaries = folder.path().join("binaries");
        let cache = folder.path().join("cache");

        let error = stage(
            &builds,
            LINUX,
            &cache,
            &Destination::Sidecars(binaries.clone()),
            &mut serving(&served, &calls),
        )
        .unwrap_err();

        assert!(error.contains(LINUX), "{error}");
        assert!(error.contains("system ffmpeg"), "{error}");
        assert_eq!(calls.get(), 0, "nothing is downloaded");
        assert!(!binaries.exists());
        assert!(!cache.exists());
    }

    #[test]
    fn a_failed_download_stages_nothing() {
        let folder = TempDir::new().unwrap();
        let (builds, _) = mac_setup(folder.path());
        let binaries = folder.path().join("binaries");
        let mut fetch = |_: &str, _: &Path| Err("curl: (22) 404".to_owned());

        let error = stage(
            &builds,
            MAC_ARM,
            &folder.path().join("cache"),
            &Destination::Sidecars(binaries.clone()),
            &mut fetch,
        )
        .unwrap_err();

        assert!(error.contains("404"), "{error}");
        assert!(!binaries.exists());
    }

    #[test]
    fn a_wrong_checksum_stages_nothing_and_deletes_the_download() {
        let folder = TempDir::new().unwrap();
        let (mut builds, served) = mac_setup(folder.path());
        // The second archive's pinned sum is wrong; the first one is right.
        builds.targets[0].archives[1].sha256 = "0".repeat(64);
        let binaries = folder.path().join("binaries");
        let cache = folder.path().join("cache");
        let calls = Cell::new(0);

        let error = stage(
            &builds,
            MAC_ARM,
            &cache,
            &Destination::Sidecars(binaries.clone()),
            &mut serving(&served, &calls),
        )
        .unwrap_err();

        assert!(error.contains("checksum"), "{error}");
        assert!(!binaries.exists(), "not even the program that was fine");
        let kept = files_in(&cache);
        assert_eq!(
            kept.len(),
            1,
            "only the good archive stays cached: {kept:?}"
        );
        assert!(kept[0].ends_with("ffmpeg.tar"), "{kept:?}");
    }

    #[test]
    fn an_archive_that_does_not_extract_stages_nothing() {
        let folder = TempDir::new().unwrap();
        let served = folder.path().join("served");
        fs::create_dir_all(&served).unwrap();
        fs::write(served.join("junk"), b"not an archive").unwrap();
        // The checksum is right: the file is what the builds file pinned.
        let builds = builds_with(
            MAC_ARM,
            vec![
                archive("fake://junk", &hash(b"not an archive"), "ffmpeg", "ffmpeg"),
                archive(
                    "fake://junk",
                    &hash(b"not an archive"),
                    "ffprobe",
                    "ffprobe",
                ),
            ],
        );
        let binaries = folder.path().join("binaries");
        let calls = Cell::new(0);

        let error = stage(
            &builds,
            MAC_ARM,
            &folder.path().join("cache"),
            &Destination::Sidecars(binaries.clone()),
            &mut serving(&served, &calls),
        )
        .unwrap_err();

        assert!(error.contains("extract"), "{error}");
        assert!(!binaries.exists());
    }

    #[test]
    fn a_program_missing_from_its_archive_stages_nothing_not_even_the_other() {
        let folder = TempDir::new().unwrap();
        let served = folder.path().join("served");
        fs::create_dir_all(&served).unwrap();
        let ffmpeg = make_tar(&served, "ffmpeg.tar", &[("ffmpeg", b"ffmpeg program")]);
        // The ffprobe archive holds something else.
        let wrong = make_tar(&served, "ffprobe.tar", &[("ffplay", b"not ffprobe")]);
        let builds = builds_with(
            MAC_ARM,
            vec![
                archive(
                    "fake://ffmpeg.tar",
                    &hash(&fs::read(ffmpeg).unwrap()),
                    "ffmpeg",
                    "ffmpeg",
                ),
                archive(
                    "fake://ffprobe.tar",
                    &hash(&fs::read(wrong).unwrap()),
                    "ffprobe",
                    "ffprobe",
                ),
            ],
        );
        let binaries = folder.path().join("binaries");
        let calls = Cell::new(0);

        let error = stage(
            &builds,
            MAC_ARM,
            &folder.path().join("cache"),
            &Destination::Sidecars(binaries.clone()),
            &mut serving(&served, &calls),
        )
        .unwrap_err();

        assert!(error.contains("ffprobe"), "{error}");
        assert!(!binaries.exists(), "ffmpeg alone must not be staged");
    }

    #[test]
    fn a_destination_that_cannot_be_created_is_an_error() {
        let folder = TempDir::new().unwrap();
        let (builds, served) = mac_setup(folder.path());
        // A file where the folder should be.
        let blocker = folder.path().join("binaries");
        fs::write(&blocker, b"a file").unwrap();
        let calls = Cell::new(0);

        let error = stage(
            &builds,
            MAC_ARM,
            &folder.path().join("cache"),
            &Destination::Sidecars(blocker),
            &mut serving(&served, &calls),
        )
        .unwrap_err();

        assert!(error.contains("cannot create"), "{error}");
    }

    #[test]
    fn a_staged_name_that_is_already_a_folder_is_an_error() {
        let folder = TempDir::new().unwrap();
        let (builds, served) = mac_setup(folder.path());
        let binaries = folder.path().join("binaries");
        fs::create_dir_all(binaries.join("ffprobe-aarch64-apple-darwin")).unwrap();
        let calls = Cell::new(0);

        let error = stage(
            &builds,
            MAC_ARM,
            &folder.path().join("cache"),
            &Destination::Sidecars(binaries),
            &mut serving(&served, &calls),
        )
        .unwrap_err();

        assert!(error.contains("cannot copy"), "{error}");
    }

    /// Whether `args` holds `flag` directly followed by `value`.
    fn has_pair(args: &[OsString], flag: &str, value: &str) -> bool {
        args.windows(2)
            .any(|pair| pair[0] == flag && pair[1] == value)
    }

    fn has(args: &[OsString], flag: &str) -> bool {
        args.iter().any(|arg| arg == flag)
    }

    #[test]
    fn an_https_download_has_time_limits_retries_and_stays_on_https() {
        let file = Path::new("/cache/ffmpeg.zip");
        let url = "https://example.com/9.1/ffmpeg.zip";
        let args = curl_args(url, file);

        assert!(has_pair(&args, "--connect-timeout", "30"), "{args:?}");
        assert!(has_pair(&args, "--max-time", "900"), "{args:?}");
        assert!(has_pair(&args, "--retry", "2"), "{args:?}");
        assert!(has_pair(&args, "--proto", "=https"), "{args:?}");
        assert!(has_pair(&args, "--proto-redir", "=https"), "{args:?}");
        for flag in ["--fail", "--location", "--silent", "--show-error"] {
            assert!(has(&args, flag), "{flag}: {args:?}");
        }
        assert!(has_pair(&args, "--output", "/cache/ffmpeg.zip"), "{args:?}");
        assert_eq!(args.last(), Some(&OsString::from(url)), "{args:?}");
    }

    #[test]
    fn a_file_url_has_the_time_limits_but_no_https_rule() {
        let file = Path::new("/cache/ffmpeg.zip");
        let url = "file:///served/ffmpeg.zip";
        let args = curl_args(url, file);

        assert!(has_pair(&args, "--connect-timeout", "30"), "{args:?}");
        assert!(has_pair(&args, "--max-time", "900"), "{args:?}");
        assert!(has_pair(&args, "--retry", "2"), "{args:?}");
        assert!(!has(&args, "--proto"), "{args:?}");
        assert!(!has(&args, "--proto-redir"), "{args:?}");
        assert!(!has(&args, "=https"), "{args:?}");
        assert_eq!(args.last(), Some(&OsString::from(url)), "{args:?}");
    }

    #[test]
    fn curl_is_never_given_an_argument_that_starts_with_a_dash() {
        let folder = TempDir::new().unwrap();
        let file = folder.path().join("out");
        let error = fetch_with_curl("--version", &file).unwrap_err();
        assert!(error.contains("refusing"), "{error}");
        assert!(!file.exists());
    }

    #[test]
    fn the_whole_job_reads_the_real_builds_file_and_refuses_a_triple_without_an_entry() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let folder = TempDir::new().unwrap();
        let dest = folder.path().join("tools");

        let error = run(&root, Some(dest.clone()), Some(LINUX)).unwrap_err();

        assert!(error.contains("system ffmpeg"), "{error}");
        assert!(!dest.exists());
        // A triple that is not one is refused before anything is read.
        assert!(run(&root, Some(dest.clone()), Some("../x")).is_err());
        // A root without builds.toml is an error naming the file.
        let error = run(folder.path(), Some(dest), Some(MAC_ARM)).unwrap_err();
        assert!(error.contains("builds.toml"), "{error}");
    }

    #[test]
    fn the_cache_name_keeps_two_targets_with_the_same_file_name_apart() {
        let one = cache_file(
            Path::new("c"),
            "https://h/arm64/1_9.1/ffmpeg.zip",
            &"a".repeat(64),
        );
        let two = cache_file(
            Path::new("c"),
            "https://h/amd64/2_9.1/ffmpeg.zip",
            &"b".repeat(64),
        );
        assert_ne!(one, two);
        assert!(one.starts_with("c"));
        assert!(one.to_string_lossy().ends_with("ffmpeg.zip"));
        // The same archive named twice is one file.
        assert_eq!(
            one,
            cache_file(
                Path::new("c"),
                "https://h/arm64/1_9.1/ffmpeg.zip",
                &"a".repeat(64)
            )
        );
    }

    #[test]
    fn calls_are_made_in_the_order_ffmpeg_then_ffprobe() {
        let folder = TempDir::new().unwrap();
        let (builds, _) = mac_setup(folder.path());
        let urls = RefCell::new(Vec::new());
        let mut fetch = |url: &str, _: &Path| {
            urls.borrow_mut().push(url.to_owned());
            Err("stop".to_owned())
        };
        let _ = stage(
            &builds,
            MAC_ARM,
            &folder.path().join("cache"),
            &Destination::Plain(folder.path().join("out")),
            &mut fetch,
        );
        // The first failure ends the run, and the first archive is ffmpeg's.
        assert_eq!(*urls.borrow(), ["fake://ffmpeg.tar"]);
    }
}
