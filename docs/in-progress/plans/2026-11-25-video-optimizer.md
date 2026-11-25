# Video optimizer Implementation Plan

**Owner:** Aureus-Lunae · **Started:** 2026-11-25

## Handoff

Decided in conversation. The user's words are quoted as they were said. The first block is the spec's handoff, copied verbatim; the second is what was decided while this plan was written. Give every implementer these lines verbatim. An implementer who wants to deviate from one stops and asks.

From the spec (2026-11-24):

- **Origin:** the user compressed five Premiere Pro exports (`~/Videos/postads test/`) with ffmpeg and asked: "if I want to make something for this similar to lunaepix?"
- **CRF 28 is the starting point:** "CRF28. Lets see the actual differences." Then "Well, try a CRF 30 on them." Measured results are in the spec's "Evidence".
- **Separate app, not inside Lunaepix:** answered "Separate app (Recommended)". Repo `~/sideprojects/video-optimizer`, same architecture as Lunaepix.
- **ffmpeg is bundled:** answered "Bundle it (Recommended)". Ruled out: "Require it installed", "Detect, else offer download".
- **Internal use only:** "It is for internal use only." Not distributed outside the organisation, so the x264 GPL obligations do not apply and no licence screen is built. If it is ever shared outside, the licence question comes back first.
- **Platforms:** "Internal means mac", then "And windows." So macOS and Windows; no Linux build.
- **Not everyone is on Apple Silicon:** asked whether everyone is on an Apple Silicon Mac, the user answered "No". So Intel Macs are built too.
- **Right column starts open:** "Column starts open".
- **Slider range:** "20 to 36". CRF 20 to 36, set by the user (HandBrake's own range is 0 to 51).
- **Quality is a slider, like HandBrake:** "the CRF can be a slider, like in handbreak". This replaces the earlier four fixed choices, which are no longer valid. The slider shows the CRF number as it moves.
- **No downscaling, ever:** "Never". There is no resolution setting; the output keeps the input's size.
- **Single video only, no Batch:** "BATCH removed dammit. I asked for single video only." No Batch mode, no queue, no file list, no mode switch.
- **Name:** "Name also removed. Just call it video optimizer." The app is called "Video optimizer" everywhere, including the command line and the repo.
- **Before/after at full size:** "Single video only, you show it next to eachother,but is it possible to also do a before after so we can see the full sized video?" Both views exist.
- **Layout like Lunaepix:** "Lunaepic also has all data in a tab that comes in from the right when selected, not below the image." Every piece of data and every setting is in the right column, none below the picture.
- **Layout:** "HTML mockup". Approved ("Ok, that looks good"), so `docs/in-progress/specs/2026-11-24-video-optimizer-mockup.html` is binding.
- **Spec's own calls (not user decisions):** default CRF 28; slider steps of 1; audio keep/remove stays a setting; the column folds away with "Hide settings".
- Put off: AV1/WebM output, HEVC output, a brew-style install.

Decided 2026-11-25:

- **Start:** "Lets start the work on this." Taken as approval of the spec; the spec moved to `docs/in-progress/specs/`.
- **An original the window cannot play:** "Show a playable copy (Recommended)". Only for such a file, the "before" is a near-lossless H.264 copy of the first 10 seconds (CRF 10), labelled "Original (converted for display)". Ruled out: "Show a message instead", "Refuse the file".
- **Plan approved:** "yes" (2026-11-25), in answer to "Does the plan capture what you want, and should it run subagent-driven or native?" with subagent-driven recommended. It runs subagent-driven, in a fresh session. The "Decided in this plan (my calls)" list under Global Constraints was shown in that message and approved with it.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A Tauri window app and a command line that re-encode one video to an H.264 MP4 at a chosen CRF (20 to 36), with a 10-second before/after comparison, never touching or overwriting anything.

**Architecture:** A Cargo workspace like Lunaepix (`~/sideprojects/Lunaepix`): `crates/core` holds all logic and runs `ffmpeg`/`ffprobe` as child processes (never through a shell); `crates/cli` and `src-tauri` are thin front ends; `crates/xtask` downloads and stages the pinned ffmpeg builds as Tauri sidecars. The window is vanilla JS in `ui/` with pure modules tested under `node --test`, wired by `ui/window.js`, as in Lunaepix. The window never sends or receives a file path: it gets names, numbers and media keys, and plays videos over a `media` URI scheme served by the backend with HTTP Range support.

**Tech Stack:** Rust 2024 (rust-version 1.90), Tauri 2 (`tauri-cli` 2.12.4, `tauri-plugin-dialog` 2.8.4), `tempfile`, `serde`/`serde_json`, `thiserror`; ffmpeg 9.1 (libx264, AAC); vanilla JS (ES modules), `node --test` (Node 22 in CI).

**Spec:** `docs/in-progress/specs/2026-11-24-video-optimizer-design.md` and its binding mock-up `docs/in-progress/specs/2026-11-24-video-optimizer-mockup.html`. Executors read both, handoff first.

**Branch:** `feature/video-optimizer`, from `main`. There is no dev server: this is a desktop app, tested with `cargo tauri dev` locally and with the CI builds on a real Mac and Windows PC. No `.idea` deployment settings, so `devsync` does not apply; no Gruntfile, so no Grunt build.

## Global Constraints

Copied from the spec:

- Output is always H.264 MP4: `-c:v libx264 -preset slow -pix_fmt yuv420p -movflags +faststart`, at the input's own size. No scale filter, ever.
- CRF: whole numbers 20 to 36, default 28, step 1. Refused everywhere else (window, CLI, encoder): 19, 37, 28.5, 0, 51, -1, `"28"` as a JSON string, NaN, an array.
- Audio: Keep (`-c:a aac -b:a 128k`) or Remove (`-an`). Default Keep.
- Odd dimensions are padded by one pixel to the next even size (`pad=`), never scaled. An even source gets no filter at all.
- One video at a time. Dropping several files uses none and says `One video at a time`.
- Sample = the first 10 seconds (the whole clip when shorter).
- Output name `optimized-<stem>.mp4` next to the original or in a chosen folder; a taken name gets `-1`, `-2`, …; nothing is ever overwritten.
- If the result is not smaller than the input, the original bytes are copied and the status says `original kept: result was not smaller`.
- A cancelled or failed encode leaves no partial file.
- Input limit: 10 GiB (10 × 1024³ bytes).
- File paths go to ffmpeg as separate process arguments, never through a shell, and never as an argument that starts with `-`.
- The window opens with CRF 28, Audio Keep, "Next to the original", column open, Before / after view. Nothing is remembered between launches.
- Bundles: macOS arm64 and x86_64 as separate ad-hoc-signed `.dmg`s (`signingIdentity: "-"`, `minimumSystemVersion` 11.0), Windows x64 NSIS installer. No Linux bundle.
- App name "Video optimizer"; identifier `io.github.aureuslunae.videooptimizer`; CLI binary `video-optimizer`.

Decided in this plan (my calls, not the user's; listed so a reviewer can see them):

- **ffmpeg builds (the spec leaves this to the plan):** FFmpeg 9.1. macOS arm64 and x86_64 from Martin Riedl's static release builds (ffmpeg.martin-riedl.de; libx264 included, SHA-256 per file, binaries signed). Windows x64 from Gyan's "essentials" build at the GitHub release tag `9.1` of `GyanD/codexffmpeg` (libx264 included). Each archive's URL and SHA-256 is pinned in `ffmpeg/builds.toml`. Martin Riedl stops making Intel release builds in January 2027: the pinned 9.1 Intel build keeps working, but a later upgrade for Intel needs another source.
- **When the result is not smaller**, the copy keeps the original's extension (`optimized-<stem>.mov` for a `.mov`), because the bytes are the original's container, not MP4.
- **"Playable by the window"** means: video codec `h264` with pixel format `yuv420p` or `yuvj420p`. Everything else gets the CRF 10 display copy (`-preset veryfast`, so it is quick to make).
- **The "before" clip has no audio**; only the sample plays sound (when Audio is Keep), so the two never play over each other.
- **"Original" in the result figures** is `input bytes × span ÷ duration` (span = 10 s or the clip's length), as the mock-up's numbers are; "Whole file, about" is `sample bytes × duration ÷ span`.
- **Sizes are shown in decimal MB** (`26.9 MB`), as the mock-up shows them.
- **A new sample is asked for 400 ms after the slider or Audio stops changing**; a newer sample request cancels the one still running.
- **While a Save runs,** Quality, Audio, Choose other, Choose folder and dropping are disabled (a drop says `Wait for the save to finish, or cancel it`), and a running sample is cancelled.
- **While the column is folded,** the status line (with the progress bar and Cancel) floats over the top of the picture, as in Lunaepix, so a running Save stays visible.
- **Fill window** makes the rest of the window `inert`, so focus stays in the comparison until Escape.
- **File chooser filter:** `mp4, mov, m4v, mkv, webm, avi`. A drop of any file is tried; ffprobe decides.
- **Class names** follow BBEM (`~/.claude/css-conventions.md`). The mock-up's own class names (`layer--before`, `rf__head`) are not binding; its look and arrangement are.

## Review Focus

Inputs the spec does not mention that will reach this app, and what a person would expect. Each has a test in the task named.

1. **A phone video with rotation metadata** (stored 1920x1080, `rotation: -90`): the size shown and kept is the displayed 1080x1920, and padding is worked out on the displayed size. — Task 3, Task 4.
2. **A file with cover art** (an MP4 whose first video stream is an `attached_pic` JPEG): the real video stream is the one encoded and probed, never the picture. — Task 3, Task 4.
3. **A container with no duration** (some WebM/MKV): progress is indeterminate, the figures that need a duration show `unknown`, nothing shows NaN. — Task 5, Task 6, Task 10.
4. **The output folder cannot be written, or the disk fills up**: Save fails with the reason and leaves no partial file and no placeholder. — Task 7.
5. **The window is closed while a Save runs**: ffmpeg is stopped and the partial file is deleted before the app exits. — Task 9.

---

## File structure

```
Cargo.toml                     workspace: crates/core, crates/cli, crates/xtask, src-tauri
.cargo/config.toml             Windows static CRT (copied from Lunaepix)
.gitignore                     target/, src-tauri/binaries/*, src-tauri/gen/, ffmpeg-cache/
ffmpeg/builds.toml             pinned ffmpeg archives per target triple
crates/core/src/
  lib.rs                       re-exports
  settings.rs                  Crf, Audio, Settings
  tools.rs                     Tools (ffmpeg/ffprobe paths), command(), file_arg()
  probe.rs                     VideoInfo, probe(), parse_probe()
  args.rs                      every ffmpeg argument list (pure)
  progress.rs                  parse_out_time(), percent()
  run.rs                       CancelFlag, run_ffmpeg()
  sample.rs                    make_before(), make_sample(), parse_psnr(), Figures
  save.rs                      candidate_name(), save()
crates/core/tests/common/clips.rs   test-clip generator (lavfi)
crates/core/tests/*.rs         integration tests against a real ffmpeg
crates/cli/src/{main.rs,args.rs}
crates/xtask/src/main.rs       `ffmpeg` command: download, verify, stage
src-tauri/                     tauri.conf.json, tauri.bundle.conf.json, tauri.macos.conf.json,
                               build.rs, capabilities/default.json, icons/
src-tauri/src/
  lib.rs                       builder, commands, drop and close handling
  session.rs                   the one loaded video, its work folder, jobs (no Tauri types)
  events.rs                    event payloads (serde)
  media.rs                     the `media` scheme: keys and Range answers
src-tauri/tests/config.rs      capability, CSP and sidecar guards
ui/
  index.html, app.css, app.js  page, styles, entry
  window.js                    wiring (listeners, invokes, events)
  state.js                     reducer: events and actions → state; status texts
  quality.js                   CRF words, valuetext
  compare.js                   playback sync, divider (reuses zoom.js)
  zoom.js                      copied from Lunaepix
  format.js                    sizes, times, fps, audio text
tests/ui/*.test.js             node tests; fake-document.js and fake-window.js copied from Lunaepix
.github/workflows/{windows.yml,macos.yml}
README.md
```

---

### Task 1: Workspace and the CRF and Audio settings

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `.cargo/config.toml` (copy from Lunaepix unchanged), `crates/core/Cargo.toml`, `crates/core/src/lib.rs`, `crates/core/src/settings.rs`
- Test: `crates/core/src/settings.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces:
  - `pub struct Crf(u8)` with `pub const MIN: u8 = 20`, `MAX: u8 = 36`, `DEFAULT: u8 = 28`; `Crf::new(value: i64) -> Result<Crf, SettingsError>`; `Crf::parse(text: &str) -> Result<Crf, SettingsError>` (ASCII digits only); `Crf::from_json(value: &serde_json::Value) -> Result<Crf, SettingsError>` (a JSON integer only); `Crf::get(self) -> u8`; `Default` = 28.
  - `pub enum Audio { Keep, Remove }`; `Audio::from_json(&Value) -> Result<Audio, SettingsError>` accepting exactly `"keep"` or `"remove"`; `Default` = Keep.
  - `pub struct Settings { pub crf: Crf, pub audio: Audio }` (Copy).
  - `pub enum SettingsError { CrfOutOfRange(i64), CrfNotWholeNumber, AudioUnknown }` (thiserror, messages: `CRF must be a whole number from 20 to 36`).

Workspace `Cargo.toml` follows Lunaepix's (resolver 3, edition 2024, rust-version 1.90, version 0.1.0, `authors = ["Aureus Lunae"]`, `[workspace.dependencies]` for `video-optimizer-core`, `thiserror = "2"`, `serde_json`, `tempfile`). Core depends on `thiserror`, `serde_json`, `tempfile`, `serde` (derive). `.gitignore`: `target/`, `src-tauri/binaries/*` with `!src-tauri/binaries/.gitkeep`, `src-tauri/gen/`, `ffmpeg-cache/`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test] fn accepts_the_ends_and_the_default() { for v in [20, 28, 36] { assert_eq!(Crf::new(v).unwrap().get() as i64, v); } }
#[test] fn rejects_values_outside_20_to_36() { for v in [19, 37, 0, 51, -1] { assert!(Crf::new(v).is_err()); } }
#[test] fn parses_only_plain_digits() {
    assert_eq!(Crf::parse("28").unwrap().get(), 28);
    for t in ["28.5", "-1", "+28", " 28", "", "２８", "28a", "0x1c"] { assert!(Crf::parse(t).is_err(), "{t}"); }
}
#[test] fn reads_only_a_json_integer() {
    assert_eq!(Crf::from_json(&json!(36)).unwrap().get(), 36);
    for v in [json!(28.5), json!("28"), json!(null), json!([28]), json!(f64::NAN), json!(37)] { assert!(Crf::from_json(&v).is_err()); }
}
#[test] fn audio_reads_keep_and_remove_only() {
    assert_eq!(Audio::from_json(&json!("remove")).unwrap(), Audio::Remove);
    for v in [json!("Keep"), json!(""), json!(1), json!(["keep"])] { assert!(Audio::from_json(&v).is_err()); }
}
#[test] fn defaults_are_crf_28_and_keep() { /* Settings::default() */ }
```

(`json!(f64::NAN)` serialises as `null`; the test still proves NaN never becomes a CRF.)

- [ ] **Step 2: Run them, expect RED** — `cargo test -p video-optimizer-core settings`: fails to compile (no `Crf`). Keep the output for the report.
- [ ] **Step 3: Implement `settings.rs`.** `from_json` uses `Value::as_i64` only when `Value::is_i64() || is_u64()`; a float that happens to be whole (`28.0`) is refused too (`is_f64`).
- [ ] **Step 4: Run, expect GREEN**, plus `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --all --check`.
- [ ] **Step 5: Commit** — `Add workspace and the CRF and audio settings`.

---

### Task 2: Finding ffmpeg and starting it safely

**Files:**
- Create: `crates/core/src/tools.rs`
- Test: `crates/core/src/tools.rs` (`#[cfg(test)]`)

**Interfaces:**
- Produces:
  - `pub struct Tools { pub ffmpeg: PathBuf, pub ffprobe: PathBuf }`
  - `Tools::beside(dir: &Path) -> Option<Tools>`: both `ffmpeg[.exe]` and `ffprobe[.exe]` exist as files in `dir`.
  - `Tools::locate() -> Result<Tools, ToolsError>`: beside `std::env::current_exe()`'s folder first (the bundled sidecars), then the folder named by env `VIDEO_OPTIMIZER_TOOLS_DIR` (tests and CI), then `PATH`. `ToolsError::NotFound` says `ffmpeg and ffprobe were not found beside the app or on the PATH`.
  - `pub(crate) fn command(program: &Path) -> std::process::Command`: stdin null, stdout/stderr piped; on Windows `creation_flags(CREATE_NO_WINDOW)` (`0x0800_0000`, as Lunaepix `crates/core/src/run.rs`), so no console flashes.
  - `pub fn file_arg(path: &Path) -> Result<OsString, PathError>`: refuses a path that is not absolute (`PathError::NotAbsolute`); returns `file:` followed by the path. The `file:` prefix stops ffmpeg reading the path as an option or as another protocol (`http:`, `concat:`), whatever the name.
  - `pub fn absolute(path: &Path) -> io::Result<PathBuf>`: `std::path::absolute` (not `canonicalize`, which gives `\\?\` paths on Windows).

- [ ] **Step 1: Write the failing tests**
  - `file_arg_prefixes_an_absolute_path`: `/videos/a b ü.mp4` → `file:/videos/a b ü.mp4`; one `OsString`, not split.
  - `file_arg_keeps_a_name_starting_with_a_dash_as_a_path`: `/v/-y.mp4` → `file:/v/-y.mp4`; the result does not start with `-`.
  - `file_arg_keeps_a_protocol_like_name_as_a_path`: `/v/concat:x.mp4` → starts with `file:/`.
  - `file_arg_rejects_a_relative_path`: `a.mp4` and `-i` → `Err(NotAbsolute)`.
  - `#[cfg(windows)] file_arg_keeps_backslashes`: `C:\Video's\a.mp4` → `file:C:\Video's\a.mp4`.
  - `beside_finds_both_tools` (tempdir with two empty executable files) / `beside_refuses_when_one_is_missing` (only `ffmpeg`).
  - `#[cfg(windows)] command_hides_the_console`: compare the flag with `windows-sys`'s `CREATE_NO_WINDOW` as Lunaepix's test does (dev-dependency `windows-sys` with `Win32_System_Threading`).
- [ ] **Step 2: RED** — `cargo test -p video-optimizer-core tools`.
- [ ] **Step 3: Implement `tools.rs`.**
- [ ] **Step 4: GREEN**, clippy, fmt.
- [ ] **Step 5: Commit** — `Find ffmpeg and pass paths to it safely`.

---

### Task 3: Probing the input

**Files:**
- Create: `crates/core/src/probe.rs`, `crates/core/tests/common/clips.rs`, `crates/core/tests/probe.rs`, `crates/core/tests/fixtures/probe/*.json`

**Interfaces:**
- Consumes: `Tools`, `command`, `file_arg`, `absolute` (Task 2).
- Produces:
  - `pub const MAX_INPUT_BYTES: u64 = 10 * 1024 * 1024 * 1024;`
  - `pub struct VideoInfo { pub path: PathBuf, pub name: String, pub bytes: u64, pub stream_index: u32, pub width: u32, pub height: u32, pub fps: Option<f64>, pub duration: Option<f64>, pub video_codec: String, pub pix_fmt: String, pub audio: Option<AudioStream> }` — `width`/`height` are the **displayed** size (swapped when the rotation is ±90 or ±270).
  - `pub struct AudioStream { pub codec: String, pub bitrate: Option<u64> }`
  - `VideoInfo::needs_display_copy(&self) -> bool` — true unless `video_codec == "h264"` and `pix_fmt` is `yuv420p` or `yuvj420p`.
  - `pub fn parse_probe(json: &str, path: PathBuf, bytes: u64) -> Result<VideoInfo, ProbeError>` (pure)
  - `pub fn probe(tools: &Tools, path: &Path) -> Result<VideoInfo, ProbeError>`: checks it is a regular file and at most `MAX_INPUT_BYTES` before starting ffprobe, then runs it with these arguments (the path last, through `file_arg`):

    ```
    -v error -print_format json -show_format -show_streams <file_arg>
    ```
  - `pub enum ProbeError { NotAFile, TooLarge { bytes: u64 }, NoVideo, Unreadable(String) }` — `Unreadable` carries ffprobe's last stderr line (trimmed, at most 300 characters).
  - Tests helper `clips::make(dir: &Path, spec: Clip) -> PathBuf` with `Clip { name, width, height, seconds, fps, audio: bool, codec: Codec::H264 | Codec::ProRes | Codec::Mpeg4, rotate: Option<i32>, cover_art: bool }`, built with ffmpeg's `testsrc2` and `sine` lavfi sources at `-preset ultrafast`. It finds ffmpeg with `Tools::locate()` and **panics** when none is found: a test never skips silently.

Stream choice: the first stream with `codec_type == "video"` whose `disposition.attached_pic` is not 1; `stream_index` is its `index`. Audio: the first `codec_type == "audio"`. Rotation: `side_data_list[].rotation` (display matrix) or `tags.rotate`. Fps: `avg_frame_rate` as `n/d`, `None` when `d == 0` or the value is not finite or not positive. Duration: `format.duration` parsed as `f64`, `None` when absent, not finite or ≤ 0.

- [ ] **Step 1: Write the failing tests**
  - Pure, from committed JSON fixtures (captured once from ffprobe 9.1 output and trimmed): `reads_size_fps_duration_and_audio` (1328x747, `24/1` → 24.0, 36.04 s, `aac` 317000) / `reports_no_audio_when_there_is_no_audio_stream`.
  - `swaps_width_and_height_for_a_90_degree_rotation` (stored 1920x1080, rotation -90 → 1080x1920) / `keeps_the_size_for_rotation_0_and_180`.
  - `skips_cover_art_and_picks_the_real_video` (stream 0 `mjpeg` with `attached_pic: 1`, stream 1 `h264` → `stream_index 1`, codec h264) / `refuses_a_file_whose_only_video_is_cover_art` → `NoVideo`.
  - `missing_or_zero_duration_is_none` (absent, `"N/A"`, `"0.000000"`) / `a_real_duration_is_kept`.
  - `zero_denominator_frame_rate_is_none` (`0/0`) / `ntsc_frame_rate_is_29_97` (`30000/1001`, within 0.001).
  - `needs_display_copy_for_prores_hevc_and_10_bit_h264` (`prores`, `hevc`, `h264`+`yuv420p10le`) / `plays_8_bit_h264_as_it_is` (`h264`+`yuv420p`, `h264`+`yuvj420p`).
  - Integration (`tests/probe.rs`, real ffprobe): `probes_a_generated_clip` (640x360, 2 s, audio) / `refuses_a_text_file_renamed_mp4` → `Unreadable` with a non-empty reason / `refuses_a_folder` → `NotAFile` / `refuses_an_audio_only_file` (an `.m4a` from `sine`) → `NoVideo` / `probes_a_name_with_spaces_quotes_and_non_ascii` (`it's a "clip" é.mp4`) / `probes_a_name_starting_with_a_dash` (`-y.mp4`).
  - `refuses_a_file_over_10_gib_without_starting_ffprobe`: a sparse file of `MAX_INPUT_BYTES + 1` bytes (`File::set_len`) → `TooLarge`; with a `Tools` whose paths point at a missing program, so starting ffprobe would give a different error / `accepts_exactly_10_gib_as_far_as_the_size_check`: a sparse file of exactly `MAX_INPUT_BYTES` is not `TooLarge` (it is `Unreadable`).
- [ ] **Step 2: RED** — `cargo test -p video-optimizer-core probe`.
- [ ] **Step 3: Implement `probe.rs`** (serde structs for the subset of ffprobe's JSON that is read; unknown fields ignored).
- [ ] **Step 4: GREEN**, clippy, fmt.
- [ ] **Step 5: Commit** — `Probe the input video`.

---

### Task 4: ffmpeg argument lists

**Files:**
- Create: `crates/core/src/args.rs`
- Test: `crates/core/src/args.rs` (`#[cfg(test)]`)

**Interfaces:**
- Consumes: `VideoInfo` (Task 3), `Settings` (Task 1), `file_arg` (Task 2).
- Produces:
  - `pub const SAMPLE_SECONDS: f64 = 10.0;`
  - `pub enum Span { Whole, Sample }`
  - `pub fn even_size(width: u32, height: u32) -> (u32, u32)`
  - `pub fn encode_args(info: &VideoInfo, settings: Settings, output: &Path, span: Span) -> Result<Vec<OsString>, PathError>`
  - `pub fn before_args(info: &VideoInfo, output: &Path) -> Result<Vec<OsString>, PathError>`
  - `pub fn psnr_args(sample: &Path, before_ref: &Path) -> Result<Vec<OsString>, PathError>`

The exact lists (`<in>`/`<out>` are `file_arg` values; `[...]` is present only when stated):

```
encode:  -hide_banner -nostdin -y -i <in> [-t 10]
         -map 0:<stream_index> [-map 0:a:0?  (Keep only)]
         -c:v libx264 -preset slow -crf <N> -pix_fmt yuv420p
         [-vf pad=<W>:<H>  (only when width or height is odd; W,H from even_size)]
         [-c:a aac -b:a 128k  (Keep)] [-an  (Remove)]
         -movflags +faststart -progress pipe:1 -nostats -f mp4 <out>

before, playable (needs_display_copy() == false):
         -hide_banner -nostdin -y -i <in> -t 10 -map 0:<stream_index> -an
         -c:v copy -movflags +faststart -f mp4 <out>
before, display copy:
         -hide_banner -nostdin -y -i <in> -t 10 -map 0:<stream_index> -an
         -c:v libx264 -preset veryfast -crf 10 -pix_fmt yuv420p [-vf pad=<W>:<H>]
         -movflags +faststart -f mp4 <out>

psnr:    -hide_banner -nostdin -i <sample> -i <before_ref>
         -lavfi [0:v][1:v]psnr -f null -
```

`-y` is safe: every `<out>` is a temporary file this app just created. For `psnr`, `before_ref` is always an even-sized H.264 file (Task 6 makes one when the before clip is a stream copy of an odd-sized source), so both inputs have the same size. ffmpeg applies the rotation itself on encode (autorotate), so padding uses the displayed size from `VideoInfo`.

- [ ] **Step 1: Write the failing tests** (on hand-built `VideoInfo` values)
  - `whole_encode_has_the_fixed_settings` (contains, in order, `-c:v libx264 -preset slow -crf 28 -pix_fmt yuv420p`, `-movflags +faststart`, `-f mp4`, and the CRF of the settings: 20 and 36 too) / `whole_encode_has_no_time_limit` (no `-t`).
  - `sample_encode_stops_at_10_seconds` (`-t 10` before `-map`) / whole has none.
  - `never_scales`: for every combination of span, audio, odd/even size, rotation, no argument contains `scale`, `-s`, or `zscale`.
  - `pads_1281x721_to_1282x722` (`-vf pad=1282:722`) / `an_even_source_gets_no_filter` (no `-vf`).
  - `pads_a_rotated_odd_source_on_its_displayed_size` (displayed 721x1281 → `pad=722:1282`).
  - `keep_maps_audio_as_aac_128k` / `remove_maps_no_audio` (`-an`, no `0:a`, no `-c:a`).
  - `maps_the_chosen_video_stream` (`stream_index 1` → `-map 0:1`) / never `0:v:0`.
  - `paths_are_one_argument_each_and_never_start_with_a_dash`: input `/v/-x "y".mp4`, output in a folder with spaces; the input appears exactly once, prefixed `file:`; no argument after `-i` starts with `-` unless it is a known option from the lists above.
  - `before_copies_a_playable_source` (`-c:v copy`, `-an`, `-t 10`) / `before_reencodes_prores_at_crf_10` (`-crf 10 -preset veryfast`, no `copy`).
- [ ] **Step 2: RED** — `cargo test -p video-optimizer-core args`.
- [ ] **Step 3: Implement `args.rs`.**
- [ ] **Step 4: GREEN**, clippy, fmt.
- [ ] **Step 5: Commit** — `Build the ffmpeg argument lists`.

---

### Task 5: Running ffmpeg with progress and cancel

**Files:**
- Create: `crates/core/src/progress.rs`, `crates/core/src/run.rs`, `crates/core/tests/run.rs`

**Interfaces:**
- Consumes: `Tools`, `command` (Task 2).
- Produces:
  - `pub fn parse_out_time(line: &str) -> Option<u64>` — microseconds from a line `out_time_us=<digits>`; `None` for `out_time_us=N/A`, negative, or any other line.
  - `pub fn percent(out_us: u64, duration: Option<f64>) -> Option<u8>` — `None` when the duration is `None`, not finite or ≤ 0; otherwise `round(out_us / 1e6 / duration × 100)` clamped to 0..=100.
  - `#[derive(Clone)] pub struct CancelFlag(Arc<AtomicBool>)` with `new()`, `cancel(&self)`, `is_cancelled(&self) -> bool`.
  - `pub fn run_ffmpeg(tools: &Tools, args: &[OsString], cancel: &CancelFlag, on_time: &mut dyn FnMut(u64)) -> Result<String, RunError>` — returns ffmpeg's stderr; reads stdout lines on the calling thread and calls `on_time` for each `out_time_us`; stderr is drained on a second thread (so a full pipe never blocks ffmpeg); checks `cancel` every 100 ms via a `try_wait` loop and on each stdout line, and on cancel kills and reaps the child.
  - `pub enum RunError { Start(io::Error), Cancelled, Failed { code: Option<i32>, reason: String } }` — `reason` is the last non-empty stderr line, at most 300 characters.

- [ ] **Step 1: Write the failing tests**
  - `parses_out_time_us` (`out_time_us=1500000` → 1500000) / `ignores_na_negative_and_other_keys` (`out_time_us=N/A`, `out_time_us=-5`, `out_time_ms=10`, `progress=end`, `""`).
  - `percent_runs_0_to_100` (0 → 0, half → 50, past the end → 100) / `percent_is_none_without_a_usable_duration` (`None`, `Some(0.0)`, `Some(-1.0)`, `Some(f64::NAN)`, `Some(f64::INFINITY)`).
  - Integration, real ffmpeg: `reports_progress_and_succeeds` (encode a 3 s generated clip; `on_time` received at least one value and the last is ≥ 2 s; `Ok`) / `a_failing_run_returns_the_exit_code_and_reason` (input `file:/nonexistent.mp4` → `Failed` with `code: Some(_)` and a non-empty reason).
  - `cancel_kills_ffmpeg_and_returns_cancelled`: encode a 60 s 1280x720 lavfi source at `-preset slow` to a temp file, cancel from another thread after the first `on_time`, expect `Cancelled` within 2 s; the child is reaped (on Unix `kill(pid, 0)` fails, as Lunaepix's worker tests check) / `an_uncancelled_run_finishes`.
- [ ] **Step 2: RED.**
- [ ] **Step 3: Implement `progress.rs` and `run.rs`.**
- [ ] **Step 4: GREEN**, clippy, fmt.
- [ ] **Step 5: Commit** — `Run ffmpeg with progress and cancel`.

---

### Task 6: The before clip, the sample, PSNR and the figures

**Files:**
- Create: `crates/core/src/sample.rs`, `crates/core/tests/sample.rs`

**Interfaces:**
- Consumes: Tasks 2 to 5.
- Produces:
  - `pub fn span(info: &VideoInfo) -> Option<f64>` — `min(10, duration)`; `None` without a duration.
  - `pub fn make_before(tools: &Tools, info: &VideoInfo, dir: &Path, cancel: &CancelFlag) -> Result<PathBuf, RunError>` — writes `before.mp4` into `dir` (the session's work folder) with `before_args`.
  - `pub struct SampleResult { pub path: PathBuf, pub bytes: u64, pub psnr: Option<f64> }`
  - `pub fn make_sample(tools: &Tools, info: &VideoInfo, settings: Settings, before: &Path, dir: &Path, name: &str, cancel: &CancelFlag) -> Result<SampleResult, RunError>` — encodes `dir/<name>.mp4` with `encode_args(.., Span::Sample)`, then PSNR against the before clip. When the before clip is a stream copy of an odd-sized source, it first makes `dir/psnr-ref.mp4` once (the display-copy arguments, i.e. padded CRF 10) and compares against that. A failed PSNR run gives `psnr: None`, not an error.
  - `pub fn parse_psnr(stderr: &str) -> Option<f64>` — the `average:` value of the last line containing `PSNR`; `average:inf` → `Some(f64::INFINITY)`.
  - `pub struct Figures { pub original_bytes: Option<u64>, pub sample_bytes: u64, pub smaller_by: Option<i32>, pub whole_bytes: Option<u64> }` and `pub fn figures(info: &VideoInfo, sample_bytes: u64) -> Figures` — `original_bytes = round(info.bytes × span ÷ duration)`; `smaller_by = round((1 − sample ÷ original) × 100)` (may be negative); `whole_bytes = round(sample_bytes × duration ÷ span)`; each `None` without a duration.

- [ ] **Step 1: Write the failing tests**
  - `parse_psnr_reads_the_average` (`[Parsed_psnr_0 @ 0x..] PSNR y:43.1 u:47.0 v:47.5 average:42.41 min:39.0 max:46.2` → 42.41) / `parse_psnr_reads_inf` / `parse_psnr_without_a_psnr_line_is_none`.
  - `figures_match_the_mock_up`: 96 900 000 bytes, 36 s, sample 11 300 000 → original 26 916 667, smaller by 58, whole 40 680 000 / `figures_without_duration_are_unknown` (all three `None`, `sample_bytes` kept).
  - `a_short_clip_uses_its_whole_length` (4 s clip: span 4, whole == sample).
  - `a_bigger_sample_gives_a_negative_saving` (sample > original → negative `smaller_by`).
  - Integration: `before_of_an_h264_clip_is_a_stream_copy` (the before file's codec is h264 and its size is within 5% of a `-c copy` cut; no audio stream) / `before_of_a_prores_clip_is_h264` (generated ProRes `.mov` → before is h264 yuv420p).
  - `sample_of_a_12_s_clip_is_10_s` (duration 10 ± 0.1 by ffprobe) / `sample_of_a_4_s_clip_is_4_s`.
  - `sample_reports_psnr_above_30_db_at_crf_28` / `psnr_falls_as_crf_rises` (CRF 36 < CRF 20 on the same clip).
  - `sample_of_an_odd_clip_is_compared_against_the_padded_reference` (1281x721 → PSNR `Some`, finite).
  - `a_cancelled_sample_returns_cancelled_and_leaves_no_sample_file`.
- [ ] **Step 2: RED.**
- [ ] **Step 3: Implement `sample.rs`.**
- [ ] **Step 4: GREEN**, clippy, fmt.
- [ ] **Step 5: Commit** — `Make the before clip, the sample and its figures`.

---

### Task 7: Saving the whole video

**Files:**
- Create: `crates/core/src/save.rs`, `crates/core/tests/save.rs`

**Interfaces:**
- Consumes: Tasks 2 to 5.
- Produces:
  - `pub const MAX_SUFFIX: u32 = 9999;`
  - `pub fn candidate_name(stem: &OsStr, extension: &str, n: u32) -> OsString` — `n == 0`: `optimized-<stem>.<ext>`; else `optimized-<stem>-<n>.<ext>`.
  - `pub struct SaveOutcome { pub path: PathBuf, pub name: String, pub bytes: u64, pub kept_original: bool }`
  - `pub enum SaveError { Run(RunError), Io(io::Error), NoFreeName }`
  - `pub fn save(tools: &Tools, info: &VideoInfo, settings: Settings, out_dir: Option<&Path>, cancel: &CancelFlag, on_progress: &mut dyn FnMut(Option<u8>)) -> Result<SaveOutcome, SaveError>`

Behaviour:
1. The folder is `out_dir`, else the original's folder.
2. ffmpeg writes to a `tempfile::Builder::new().prefix(".optimized-").suffix(".part")` file in that folder (same filesystem, so the final rename is atomic). `on_progress` gets `percent(out_us, info.duration)`.
3. If the temporary file is not smaller than `info.bytes`, it is replaced by a copy of the original's bytes (`fs::copy` into a second temporary file in the folder), the extension becomes the original's (lowercased; `mp4` when it has none), and `kept_original = true`.
4. The temporary file is moved to the first free `candidate_name(stem, ext, n)` for n = 0, 1, … `MAX_SUFFIX` with `NamedTempFile::persist_noclobber`; an `AlreadyExists` error tries the next n, any other error stops. All taken → `NoFreeName`.
5. On cancel, failure, or any error, the temporary files are dropped (deleted); nothing else in the folder changes.

- [ ] **Step 1: Write the failing tests**
  - `names_the_first_output` (`a` → `optimized-a.mp4`; `n = 2` → `optimized-a-2.mp4`) / `keeps_a_name_with_spaces_and_non_ascii` (`Showreel V1 é`).
  - Integration (real ffmpeg, a high-bitrate 3 s 640x360 clip made at CRF 0 so CRF 28 is smaller):
    - `writes_optimized_next_to_the_original` (`kept_original == false`, output size < input, h264, `+faststart`: the `moov` atom comes before `mdat`, read from the file's first boxes) / `the_original_is_byte_identical_afterwards`.
    - `a_taken_name_gets_a_number`: existing `optimized-a.mp4` and `optimized-a-1.mp4` with known bytes → output `optimized-a-2.mp4`; both existing files are byte-identical afterwards / `the_first_free_name_is_used_when_nothing_is_taken`.
    - `writes_into_the_chosen_folder` / the original's folder gets no new file.
    - `keeps_the_original_when_the_result_is_not_smaller`: a source made at CRF 40 then saved at CRF 20 → `kept_original`, output bytes equal the input bytes, extension of the input (`.mov` source → `optimized-a.mov`) / `writes_the_encode_when_it_is_smaller` (`kept_original == false`).
    - `keeps_the_size`: 1280x720 in → 1280x720 out / `pads_an_odd_source`: 1281x721 → 1282x722.
    - `keeps_a_rotated_clip_upright`: a 640x360 clip tagged rotate 90 → output 360x640 as displayed.
    - `remove_audio_gives_no_audio_stream` / `keep_audio_gives_an_aac_stream`.
    - `cancel_deletes_the_partial_output`: cancel after the first progress → `Run(Cancelled)`; the folder lists exactly what it had before (no `.part`, no `optimized-*`) / `a_finished_save_is_not_deleted_by_a_later_cancel` (cancel the flag after `Ok`; the file is still there).
    - `an_ffmpeg_failure_leaves_nothing`: a `Tools` whose `ffmpeg` is a script that writes some bytes to its last argument's path and exits 1 (Unix only, `#[cfg(unix)]`) → `Run(Failed)` with the code; folder unchanged / `exit_0_returns_done`.
    - `an_unwritable_folder_fails_cleanly` (`#[cfg(unix)]`, folder mode 0o555) → `Io` or `Run(Failed)`; folder unchanged.
    - `progress_runs_to_100` (last value `Some(100)`) / `progress_without_a_duration_is_none` (on a hand-built `VideoInfo` with `duration: None`, every value is `None`).
    - `a_name_starting_with_a_dash_is_saved`: input `-y.mp4` → `optimized--y.mp4`.
- [ ] **Step 2: RED.**
- [ ] **Step 3: Implement `save.rs`.**
- [ ] **Step 4: GREEN**, clippy, fmt.
- [ ] **Step 5: Commit** — `Save the whole video without overwriting anything`.

---

### Task 8: The command line

**Files:**
- Create: `crates/cli/Cargo.toml` (bin `video-optimizer`), `crates/cli/src/main.rs`, `crates/cli/src/args.rs`, `crates/cli/tests/cli.rs`

**Interfaces:**
- Consumes: `Crf::parse`, `Tools::locate`, `probe`, `save` (Tasks 1 to 7).
- Produces: `video-optimizer <file> [--crf N] [--no-audio] [--out DIR]`.

Argument rules (hand-written parser, no clap, as Lunaepix's `crates/cli/src/args.rs`): exactly one file; `--crf` takes the next argument through `Crf::parse`; `--out` must be an existing folder; `--` ends options so a file named `-x.mp4` can be given; an unknown option, a second file, or a missing value is a usage error. Exit codes: 0 done (including "original kept"), 1 the video failed (probe, encode), 2 usage. Output on stdout, one line: `optimized-a.mp4: 68.3 MB -> 5.8 MB (91% smaller)` or `optimized-a.mov: original kept: result was not smaller`. Progress goes to stderr as `\r42%` only when stderr is a terminal (`std::io::IsTerminal`). Ctrl-C is not handled specially: ffmpeg gets the same signal and the `.part` file is removed by `tempfile` only on a normal exit; the README says so.

- [ ] **Step 1: Write the failing tests** (`args.rs` unit tests, and `tests/cli.rs` running the built binary with `env!("CARGO_BIN_EXE_video-optimizer")` and `VIDEO_OPTIMIZER_TOOLS_DIR` unset so `PATH` ffmpeg is used)
  - `accepts_crf_20_28_36` / `rejects_crf_19_37_28_5_minus_1_and_text` (exit 2, stderr names the range, no file written).
  - `defaults_to_crf_28_and_keep` / `no_audio_sets_remove`.
  - `refuses_two_files` / `refuses_a_missing_out_folder`.
  - `a_dash_name_after_double_dash_is_a_file` (`-- -x.mp4` works) / `a_dash_name_without_double_dash_is_an_unknown_option` (exit 2, nothing written).
  - `optimizes_a_clip` (exit 0, `optimized-a.mp4` exists, stdout line matches `^optimized-a\.mp4: [0-9.]+ MB -> [0-9.]+ MB \([0-9]+% smaller\)$`) / `refuses_a_text_file` (exit 1, stderr has a reason, nothing written).
- [ ] **Step 2: RED.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: GREEN**, clippy, fmt.
- [ ] **Step 5: Commit** — `Add the video-optimizer command line`.

---

### Task 9: The app backend (session, commands, events)

**Files:**
- Create: `src-tauri/Cargo.toml` (lib `video_optimizer_app_lib`, rlib only), `src-tauri/build.rs`, `src-tauri/tauri.conf.json`, `src-tauri/capabilities/default.json`, `src-tauri/src/main.rs`, `src-tauri/src/lib.rs`, `src-tauri/src/session.rs`, `src-tauri/src/events.rs`, `src-tauri/icons/*` (generated with `cargo tauri icon` from a plain placeholder SVG; the real icon is not part of this plan), `src-tauri/tests/config.rs`

**Interfaces:**
- Consumes: the whole core.
- Produces, for the window (`window.__TAURI__.core.invoke` / `event.listen`):

| Command | Arguments | Answer |
|---|---|---|
| `choose_video` | — | `null`; the result comes as a `video` or `notice` event |
| `choose_folder` | — | `{ "folder": "Exports" }` or `{ "folder": null }` when cancelled (the folder's name only) |
| `clear_folder` | — | `null` (back to "Next to the original") |
| `make_sample` | `{ "video": 3, "request": 7, "crf": 28, "audio": "keep" }` | `null`; result as a `sample` event |
| `save` | `{ "video": 3, "crf": 28, "audio": "keep" }` | `null`; results as `save-progress` / `save-done` / `save-failed` events |
| `cancel_save` | — | `null` |

`crf` and `audio` arrive as `serde_json::Value` and go through `Crf::from_json` / `Audio::from_json`; a refusal answers the invoke with an error string and starts nothing. A `video` id that is not the current one is ignored (`null`, nothing starts).

Events, with real payloads:

```json
"video":         {"video": 3, "name": "Showreel V1.mp4", "width": 3840, "height": 2160,
                  "fps": 24.0, "duration": 36.04, "bytes": 96900000,
                  "audio": {"codec": "aac", "bitrate": 317000}, "displayCopy": false}
                 (no audio: "audio": null; no duration: "duration": null; unknown fps: "fps": null)
"before-ready":  {"video": 3}
"notice":        {"kind": "several-files"} | {"kind": "saving"}
                 | {"kind": "refused", "name": "notes.mp4", "reason": "Invalid data found when processing input"}
                 | {"kind": "too-large", "name": "huge.mov"}
                 | {"kind": "failed", "reason": "…"}           (before clip failed)
"sample":        {"video": 3, "request": 7, "crf": 28, "originalBytes": 26916667, "sampleBytes": 11300000,
                  "smallerBy": 58, "wholeBytes": 40680000, "psnr": 42.41, "identical": false}
                 (no duration: originalBytes, smallerBy and wholeBytes are null; psnr is null when unknown;
                  an infinite PSNR is sent as "psnr": null, "identical": true, because JSON has no infinity)
"sample-failed": {"video": 3, "request": 7, "reason": "…"}
"save-progress": {"percent": 42}   |   {"percent": null}   (indeterminate)
"save-done":     {"name": "optimized-Showreel V1.mp4", "bytes": 40800000, "inputBytes": 96900000, "keptOriginal": false}
"save-failed":   {"reason": "No space left on device"}
"save-cancelled": {}
```

Every `sample` payload has the same keys; a missing value is `null`, never left out. The window builds media URLs with `convertFileSrc('<video>-before', 'media')` and `convertFileSrc('<video>-sample-<request>', 'media')` (Task 10).

`session.rs` holds no Tauri types (so it is unit-tested without a runtime): `Session { tools, video: Option<Loaded>, next_video: u64, folder: Option<PathBuf>, sample_job: Option<(u64, CancelFlag)>, save_job: Option<CancelFlag> }`; `Loaded { id, info, work: tempfile::TempDir, before: Option<PathBuf>, samples: HashMap<u64, PathBuf> }`. Rules:

- `load(paths: &[PathBuf])`: refused with `saving` while `save_job` is set; with `several-files` when `paths.len() != 1`; probing happens before the current video is replaced, so a refused file leaves the current video (spec). On success the old `Loaded` is dropped (its work folder deleted), a new id is given, and the before clip is made on a background thread (`before-ready` when done).
- `make_sample`: cancels the running sample job, starts a new one after the before clip exists; when finished, keeps only the newest sample file per video (older ones deleted).
- `save`: cancels the running sample; sets `save_job`; runs on a thread; clears `save_job` when done.
- Drop: `on_window_event` for `WindowEvent::DragDrop(DragDropEvent::Drop { paths, .. })` calls `load` (as Lunaepix `src-tauri/src/lib.rs:398`).
- Close: on `WindowEvent::CloseRequested`, if a save runs, cancel it and wait for its thread (at most 5 s) before letting the window close, so the `.part` file is deleted.
- `choose_video` uses `tauri-plugin-dialog` from Rust with the filter `mp4, mov, m4v, mkv, webm, avi`; `choose_folder` its folder picker.

`tauri.conf.json`: productName `Video optimizer`, identifier `io.github.aureuslunae.videooptimizer`, window `main` titled `Video optimizer`, 1100x720, min 640x480, `dragDropEnabled: true`, `withGlobalTauri: true`, `frontendDist: "../ui"`, bundle targets `["nsis"]` (no Linux or MSI target), CSP:

```
default-src 'self'; script-src 'self'; style-src 'self'; img-src 'self'; media-src media: http://media.localhost; font-src 'self'; connect-src ipc: http://ipc.localhost; object-src 'none'; base-uri 'none'; form-action 'none'; frame-src 'none'
```

`build.rs` declares the six commands through `AppManifest::commands`, and the capability grants exactly `allow-choose-video`, `allow-choose-folder`, `allow-clear-folder`, `allow-make-sample`, `allow-save`, `allow-cancel-save`, `core:event:allow-listen`, `core:event:allow-unlisten` (Lunaepix pattern; no dialog, fs, shell or opener permission for the window).

- [ ] **Step 1: Write the failing tests**
  - `session.rs` unit tests with real ffmpeg and generated clips: `loading_a_second_video_replaces_the_first` (id 2, first work folder deleted) / `dropping_two_files_loads_neither` (`several-files`, current video unchanged).
  - `a_refused_file_keeps_the_current_video` (text file `.mp4` → `refused` with a reason; current id unchanged) / `a_valid_file_is_loaded`.
  - `a_drop_while_saving_is_refused` (`saving`) / `a_drop_after_the_save_is_loaded`.
  - `a_newer_sample_cancels_the_older` (request 1's flag is cancelled when request 2 starts; only request 2's file remains) / `a_single_request_finishes`.
  - `make_sample_refuses_a_bad_crf_and_starts_nothing` (`37`, `"28"`, `28.5`, `[28]`) / `make_sample_accepts_20_and_36`.
  - `a_stale_video_id_starts_nothing` / the current id starts a job.
  - `closing_during_a_save_deletes_the_partial_file` (call the close handler's session method; the folder has no `.part` afterwards) / `closing_after_a_save_keeps_the_file`.
  - `tests/config.rs`: `capability_grants_exactly_the_six_commands_and_listen` / `capability_refuses_an_extra_permission` (the checker run on a fixture JSON with `dialog:default` added reports it); `csp_allows_media_only_from_the_media_scheme` (parse the CSP string into directives; `media-src` is exactly `media: http://media.localhost`; no directive contains `'unsafe-inline'` or `'unsafe-eval'` or `*`) / the same check fails on a fixture CSP with `media-src *`.
  - `events.rs`: `sample_payload_always_has_every_key` (serialise one with all `None`: every key present as `null`) / `a_full_payload_matches_the_example` (the JSON above).
- [ ] **Step 2: RED.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: GREEN** — `cargo test --workspace`, clippy, fmt. Then `cargo tauri dev` starts a window showing `ui/index.html` (an empty page is fine at this point) with no console errors.
- [ ] **Step 5: Commit** — `Add the app backend`.

---

### Task 10: The `media` scheme with Range

**Files:**
- Create: `src-tauri/src/media.rs`
- Modify: `src-tauri/src/lib.rs` (register `media` with `register_asynchronous_uri_scheme_protocol`, as Lunaepix registers `preview`)

**Interfaces:**
- Consumes: `Session` (Task 9).
- Produces:
  - `pub enum MediaKey { Before { video: u64 }, Sample { video: u64, request: u64 } }` and `pub fn parse_key(uri_path: &str) -> Option<MediaKey>` — exactly `^/(\d{1,10})-before$` or `^/(\d{1,10})-sample-(\d{1,10})$`, ASCII digits only, on the path as received (a percent-encoded path is never a key). Only the current video's before clip and its newest sample are ever served; anything else is `404` with an empty body.
  - `pub enum RangeAnswer { Full, Partial { start: u64, end: u64 }, Unsatisfiable }` and `pub fn parse_range(header: Option<&str>, len: u64) -> RangeAnswer` — supports one range `bytes=a-b`, `bytes=a-`, `bytes=-n`; several ranges or a malformed header → `Full`.
  - `pub const MAX_CHUNK: u64 = 4 * 1024 * 1024;` — a `Partial` answer is at most this long (`bytes=0-` gets the first 4 MiB), which a media element handles by asking again.
  - Responses: `200` with `Accept-Ranges: bytes` and the whole file for `Full`; `206` with `Content-Range: bytes a-b/len` for `Partial`; `416` with `Content-Range: bytes */len` for `Unsatisfiable`; `Content-Type: video/mp4` on every 200/206.

- [ ] **Step 1: Write the failing tests**
  - `parses_before_and_sample_keys` / `rejects_other_paths` (`/3-before/`, `/03x-before`, `/%33-before`, `/3-sample-`, `/../3-before`, `/3-sample-7-8`, 11 digits).
  - `range_0_to_99` → `Partial 0..=99` / `range_from_500_on_a_1000_byte_file` → `Partial 500..=999` / `suffix_range_last_100` → `Partial 900..=999`.
  - `a_range_past_the_end_is_unsatisfiable` (`bytes=1000-` on 1000 bytes) / `a_range_ending_past_the_end_is_cut_to_the_end` (`bytes=990-2000` → `990..=999`).
  - `malformed_or_multiple_ranges_answer_full` (`bytes=a-b`, `bytes=0-1,5-6`, `items=0-1`, `bytes=5-1`) / `no_header_answers_full`.
  - `an_open_range_is_capped_at_4_mib` (`bytes=0-` on 10 MiB → `0..=4194303`) / a closed range under the cap is not cut.
  - `serves_only_the_current_videos_files`: old video id → 404; a sample request that was replaced → 404 / the newest sample → 206.
- [ ] **Step 2: RED.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: GREEN**, clippy, fmt.
- [ ] **Step 5: Commit** — `Serve the before clip and the sample with Range`.

---

### Task 11: Window logic (pure modules)

**Files:**
- Create: `ui/state.js`, `ui/quality.js`, `ui/format.js`, `ui/compare.js`; copy `ui/zoom.js` from Lunaepix (keep `zoomLimits`, `toActual`, `toFit`, `pan`, `divider`, `isUsableSize`; drop what nothing uses, with the tests that cover it)
- Test: `tests/ui/state.test.js`, `tests/ui/quality.test.js`, `tests/ui/format.test.js`, `tests/ui/compare.test.js`, `tests/ui/zoom.test.js` (copied, trimmed to the kept functions)

**Interfaces:**
- Produces:
  - `quality.js`: `CRF_MIN = 20`, `CRF_MAX = 36`, `CRF_DEFAULT = 28`; `crfWord(crf)` with the mock-up's words exactly: ≤22 `Near lossless, very big`, ≤24 `High quality`, ≤27 `Good`, 28 `Recommended for the web`, ≤30 `Smaller`, else `Smallest, check the picture`; `crfValueText(crf)` → `CRF 28, Recommended for the web`; `readCrf(value)` → a whole number 20..36 or `null` (from the range input's string).
  - `format.js`: `formatMB(bytes)` decimal: `26.9 MB`, below 1 MB `0.4 MB`, ≥ 1000 MB `1.2 GB`, not a size → `unknown`; `formatLength(seconds)` `0:36`, `1:02:03`, `null` → `unknown`; `formatFps(fps)` `24 fps`, `29.97 fps`, `null` → `unknown`; `formatAudio(audio)` `AAC, 317 kbps`, no bitrate `AAC`, `null` → `None`; `formatPsnr(psnr, identical)` `42.4 dB`, identical → `identical`, `null` → `unknown`; `formatPosition(seconds, total)` `0:03 / 0:10`.
  - `state.js`: `initialState()` → `{ video: null, crf: 28, audio: 'keep', folder: null, view: 'before-after', columnOpen: true, fill: false, zoom: 'fit', request: 0, sample: null, sampling: false, saving: null, status: '' }`; `reduce(state, action)`, where an action is `{ type, ... }` for each backend event (`video`, `before-ready`, `notice`, `sample`, `sample-failed`, `save-progress`, `save-done`, `save-failed`, `save-cancelled`) and each user action (`crf`, `audio`, `folder`, `view`, `column`, `fill`, `zoom`, `save-started`); `sampleRequest(state)` → the `make_sample` arguments, or `null` when there is no video, no before clip yet, or a save runs; `canSave(state)`; `controlsDisabled(state)`.
  - Status texts, exactly (the window composes them, the backend sends data):
    - `video` received: `Preparing the original…` when `displayCopy`, else `Making the sample…`
    - `sample`: `Sample ready. Save encodes the whole video.`
    - `sample-failed`: `The sample failed: <reason>`
    - `notice several-files`: `One video at a time`
    - `notice saving`: `Wait for the save to finish, or cancel it`
    - `notice refused`: `<name> cannot be used: <reason>`
    - `notice too-large`: `<name> is larger than 10 GiB`
    - `save-progress`: `Saving… 42%` / `Saving…`
    - `save-done`: `Saved <name> (<formatMB(bytes)>, <n>% smaller)`; kept: `Saved <name>: original kept: result was not smaller`
    - `save-failed`: `Save failed: <reason>`; `save-cancelled`: `Save cancelled`
  - `compare.js`: `driftCorrection(originalTime, sampleTime, fps)` → the time to seek the sample to when `|diff| > 1 / (fps ?? 30)`, else `null`; `dividerClip(value)` → `inset(0 0 0 <v>%)` for the sample layer using Lunaepix's `divider()` to clamp (0 → only the sample shows, 100 → only the original); a value `divider()` rejects → `null` (the page keeps its last clip).

- [ ] **Step 1: Write the failing tests** (`node:test`, as Lunaepix's `tests/ui/*.test.js`)
  - `readCrf accepts 20, 28, 36` / `readCrf refuses '19', '37', '28.5', '', 'abc', NaN, ['28']` (→ `null`).
  - `crfWord gives the mock-up's words at 20, 23, 26, 28, 30, 36` / `crfValueText at 28 is "CRF 28, Recommended for the web"`.
  - `a crf action stores the value and asks a new sample (request + 1)` / `a crf action with 37 changes nothing` (state identical).
  - `a stale sample event is ignored` (request 6 when 7 is current, or another video id) / `the current sample is stored and the status says Sample ready…`.
  - `a video event resets the sample and keeps crf, audio and folder` / `a notice refused keeps the current video`.
  - `several-files gives "One video at a time"` / `a single video event does not show it`.
  - `save-progress 42 gives "Saving… 42%"`, `null gives "Saving…"` (never `NaN`) / percent 101 or `'x'` is shown as `Saving…`.
  - `save-done kept gives "… original kept: result was not smaller"` / not kept gives the size and percentage.
  - `sampleRequest is null while saving, before before-ready, without a video` / returns `{video, request, crf, audio}` otherwise.
  - `controlsDisabled while saving` / enabled after `save-done`, `save-failed`, `save-cancelled`.
  - `column false hides the column and keeps crf, audio, folder` / `column true restores it`.
  - `formatMB 26 916 667 → 26.9 MB`, `formatLength(36.04) → 0:36`, `formatFps(29.97002997) → 29.97 fps` / each with `null`, `NaN`, `-1` → `unknown` (audio `None`).
  - `driftCorrection seeks when 0.05 s apart at 24 fps` / `does not seek when 0.02 s apart`; fps `null` uses 30.
  - `dividerClip(0)`, `(50)`, `(100)` / `(-1)` and `(101)` clamp to 0 and 100, `('abc')` → `null`.
- [ ] **Step 2: RED** — `node --test "tests/ui/*.test.js"`.
- [ ] **Step 3: Implement.**
- [ ] **Step 4: GREEN.**
- [ ] **Step 5: Commit** — `Add the window's state, quality and comparison logic`.

---

### Task 12: The page, its wiring and its styles

**Files:**
- Create: `ui/index.html`, `ui/app.css`, `ui/app.js`, `ui/window.js`; copy `tests/ui/fake-document.js` and `tests/ui/fake-window.js` from Lunaepix (extend the fake element with `play()`, `pause()`, `currentTime`, `paused`, `muted` for `<video>`)
- Test: `tests/ui/window.test.js`

**Interfaces:**
- Consumes: Task 11's modules; the commands and events of Tasks 9 and 10.
- Produces: `startWindow({ doc, tauri, timers, ResizeObserver, log })` in `window.js` (Lunaepix's signature), called by `app.js` with the real objects.

## Layout

The approved mock-up `docs/in-progress/specs/2026-11-24-video-optimizer-mockup.html` is binding: compare against it, do not redraw it. In words:

- The picture fills the window left of the column. Above it, one bar: the view switch (`Before / after`, `Side by side`, buttons with `aria-pressed`) and the buttons `Fill window` and `Zoom 100%` (becomes `Fit to window` while zoomed). Under it only the transport: one `Play`/`Pause` button, the position bar (range input named `Position in the 10 second sample`) and `0:03 / 0:10`.
- Before / after: original and sample stacked at full size; the sample layer clipped by the divider; a visible handle; the range input `Before and after divider` (0 to 100, starts at 50) over the picture. Tags `Original` (or `Original (converted for display)`) top left, `Sample, CRF 28` top right.
- Side by side: the two next to each other, each with its tag top left.
- The column on the right, top to bottom: `Hide settings`; a scrolling part (`role="region"`, `aria-label="Settings"`, `tabindex="0"`) with the groups **File** (Name, Size `3840 x 2160`, Frame rate, Length, Audio; `Choose other`), **Quality** (`CRF 28` + word; the range input `Quality (CRF)` 20 to 36, step 1, `aria-valuetext`; tick marks at 20, 24, 28, 32, 36; `Better quality, bigger file` / `Smaller file` under the ends), **Audio** (a `<select>` with a `<label>`: Keep, Remove), **Save to** (`Choose folder`, then `Next to the original` or the folder's name, and a `Clear` button when a folder is set), **Result (first 10 seconds)** (Original, Optimized, Smaller by, Whole file about, Quality (PSNR), then `Above 40 dB is hard to see.`); and a bottom part that does not scroll: the status line (`aria-live="polite"`) with the progress bar (`<progress>`, no `value` attribute while indeterminate) and `Cancel` while saving, then `Save`.
- With no video loaded, the picture area shows `Drop a video here, or choose one` with a `Choose video` button; the File and Result groups each show `No video yet` instead of their figures, and Save is disabled.
- `Hide settings` folds the column (it slides; no animation under `prefers-reduced-motion: reduce`) and shows `Show settings` over the top right of the picture; the status line then floats over the top of the picture.
- `Fill window` puts the comparison over the whole window; the rest of the page gets `inert`; Escape closes it and focus returns to `Fill window`.
- Narrowest window (640 px wide): the column keeps 280 px and the picture shrinks; no horizontal scrolling.

Class names (BBEM, camelCase, modifiers as `-sibling` classes; JS toggles only modifier classes): `stage`, `stage__bar`, `stage__picture`, `stage__layer -before`/`-after`, `stage__tag`, `stage__handle`, `stage__divider`, `stage__transport`, `stage -full`, `stage -zoomed`, `settings`, `settings__top`, `settings__scroll`, `settings__group`, `settings__bottom`, `settings -hidden`, `quality__head`, `quality__value`, `quality__word`, `quality__ends`, `status`, `status -floating`, `button`, `button -primary`. Text at least 16px, secondary text at least 14px; every control has a visible focus style; contrast checked in APCA (body text Lc 75 or more against its background); targets at least 24x24 px.

Behaviour wired in `window.js`:

- Both `<video>` elements: `preload="auto"`, `playsinline`, no `autoplay`, no `controls`; the original is always `muted`; the sample is muted when Audio is Remove. Play/pause/seek act on both; on every `timeupdate` of the original and after every seek, `driftCorrection` is applied to the sample.
- The slider's `input` event updates the number, the word, `aria-valuetext` and the sample tag at once; the sample is asked 400 ms after the last `input`/`change` of the slider or Audio (one timer, reset on each change).
- Zoom 100%: `zoom.js` `toActual` / `toFit`, applied to both layers together; dragging moves around the picture (`pan`), with pointer capture.
- New `sample` event → the sample `<video>` gets the new URL and is seeked to the original's current time; playback state is kept.

- [ ] **Step 1: Write the failing tests** (`tests/ui/window.test.js`, driving `startWindow` with the fakes, as Lunaepix's `window.test.js`)
  - `moving the slider updates the number, word and valuetext at once` / `it asks no sample until 400 ms after the last move` (one `make_sample` after the timer, none before; two moves → one request).
  - `the slider stops at 20 and 36` (input `min`/`max`/`step` attributes are `20`, `36`, `1`; a fake value `37` is read as no change) / `a value of 28 sends crf 28`.
  - `a video event shows the File group and asks a sample after before-ready` / `no sample is asked before before-ready`.
  - `dropping two files shows "One video at a time"` (fake `notice` event) / a single file does not.
  - `Hide settings folds the column, shows Show settings and floats the status` / `Show settings restores the column with the same crf, audio and folder` and moves focus to `Hide settings`.
  - `Fill window makes the rest inert and Escape restores it, focus back on Fill window` / Escape without fill does nothing.
  - `the divider input moves the clip` (`50` → `inset(0 0 0 50%)`) / `"abc"` leaves the last clip.
  - `play starts both videos, pause stops both, seeking the bar seeks both` / `a drift of 0.02 s at 24 fps is left alone`.
  - `Zoom 100% shows one video pixel per screen pixel` (both layers get `scale(1)` relative to the natural size via `toActual`) / `Fit to window shows the whole frame again`.
  - `Save disables Quality, Audio, Choose other, Choose folder and shows Cancel and the progress` / `save-done enables them again and hides Cancel`.
  - `the original video is always muted; the sample is muted only with Audio Remove`.
  - `no element is written with innerHTML` (the fake document records markup writes; the count is 0 after a full session of events) / the fake records a markup write when one is made (a fixture call), so the check can fail.
- [ ] **Step 2: RED.**
- [ ] **Step 3: Implement** `index.html`, `app.css`, `app.js`, `window.js`. Texts reach the page with `textContent` only.
- [ ] **Step 4: GREEN**, then run `cargo tauri dev`, load `~/Videos/postads test/Padded banner 1328x747.mp4` and compare the window with the mock-up side by side (screenshot both into the report). Check by hand: the slider with the keyboard (arrows one step, Home 20, End 36), the divider with the keyboard, Fill window and Escape, Zoom 100% and dragging, Hide/Show settings, a Save to completion, a Save cancelled, the 640 px wide window.
- [ ] **Step 5: Commit** — `Add the window`.

---

### Task 13: Bundled ffmpeg (xtask) and the bundle config

**Files:**
- Create: `ffmpeg/builds.toml`, `crates/xtask/Cargo.toml`, `crates/xtask/build.rs` (host triple, as Lunaepix), `crates/xtask/src/main.rs`, `src-tauri/tauri.bundle.conf.json`, `src-tauri/tauri.macos.conf.json`, `src-tauri/binaries/.gitkeep`
- Modify: `src-tauri/tests/config.rs`

**Interfaces:**
- Produces:
  - `cargo run --locked -p video-optimizer-xtask -- ffmpeg [--dest DIR]`: for the target triple (`TAURI_ENV_TARGET_TRIPLE`, else the host triple), downloads each archive in `ffmpeg/builds.toml` into `ffmpeg-cache/` (skipped when a cached file's SHA-256 already matches), checks the SHA-256 (refuses on mismatch and deletes the download), extracts it (`tar -xf`, which reads `.zip` on macOS and Windows 10+), and copies the two programs to `src-tauri/binaries/ffmpeg-<triple>[.exe]` and `ffprobe-<triple>[.exe]` (Tauri's `externalBin` names), mode 0755. With `--dest DIR` it copies them as plain `ffmpeg[.exe]`/`ffprobe[.exe]` into `DIR` instead (for CI tests, via `VIDEO_OPTIMIZER_TOOLS_DIR`). A triple with no entry (Linux) is an error that says to use the system ffmpeg.
  - Downloads with `curl --fail --location --silent --show-error --output <file> <url>` as a child process (no shell); SHA-256 with the `sha2` crate (the only new dependency, in the xtask only).
  - `builds.toml` shape (the implementer fills in the real URLs and SHA-256 values from the vendors' pages, using the timestamped release paths, not the `redirect/latest` ones, and the vendors' published `.sha256` files):

    ```toml
    version = "9.1"

    [[target]]
    triple = "aarch64-apple-darwin"
    archives = [
      { url = "https://ffmpeg.martin-riedl.de/download/macos/arm64/<timestamp>_9.1/ffmpeg.zip",  sha256 = "…", program = "ffmpeg" },
      { url = "https://ffmpeg.martin-riedl.de/download/macos/arm64/<timestamp>_9.1/ffprobe.zip", sha256 = "…", program = "ffprobe" },
    ]

    [[target]]
    triple = "x86_64-apple-darwin"     # .../macos/amd64/<timestamp>_9.1/...
    [[target]]
    triple = "x86_64-pc-windows-msvc"
    archives = [
      { url = "https://github.com/GyanD/codexffmpeg/releases/download/9.1/ffmpeg-9.1-essentials_build.zip", sha256 = "…", program = "ffmpeg", inner = "bin/ffmpeg.exe" },
      # the same archive gives ffprobe: program = "ffprobe", inner = "bin/ffprobe.exe"
    ]
    ```

    The exact asset name of the Windows zip is read from the release page; it was not visible when this plan was written.
  - `tauri.bundle.conf.json`: `"build": { "beforeBuildCommand": "cargo run --locked -p video-optimizer-xtask -- ffmpeg" }`, `"bundle": { "externalBin": ["binaries/ffmpeg", "binaries/ffprobe"] }`.
  - `tauri.macos.conf.json`: `targets ["app", "dmg"]`, `signingIdentity "-"`, `minimumSystemVersion "11.0"`.

- [ ] **Step 1: Write the failing tests**
  - xtask unit tests: `sidecar_names_for_each_triple` (`ffmpeg-aarch64-apple-darwin`, `ffprobe-x86_64-pc-windows-msvc.exe`) / `rejects_an_empty_or_non_triple` (`""`, `../x`, `abc`, as Lunaepix's `sidecar_name`).
  - `builds_toml_has_all_three_triples_with_64_hex_sha256` / the checker refuses a fixture with a 63-character hash and one with an `http://` URL.
  - `a_wrong_checksum_is_refused_and_the_file_deleted` (a local `file://` archive with a wrong sum) / `the_right_checksum_is_accepted`.
  - `config.rs`: `bundle_config_ships_ffmpeg_and_ffprobe` (`externalBin` equals exactly the two) / the check fails on a fixture without `ffprobe`; `macos_config_signs_ad_hoc_for_11_0` / fails on `signingIdentity: null`.
- [ ] **Step 2: RED.**
- [ ] **Step 3: Implement**, fill in `builds.toml` with the real URLs and checksums.
- [ ] **Step 4: GREEN**; then on this Linux machine `cargo run -p video-optimizer-xtask -- ffmpeg --dest /tmp/x` fails with the Linux message, and with `TAURI_ENV_TARGET_TRIPLE=x86_64-pc-windows-msvc` it downloads, verifies and stages the Windows programs (proves the download and checksum path without a Windows machine).
- [ ] **Step 5: Commit** — `Stage the pinned ffmpeg builds as sidecars`.

---

### Task 14: CI builds, README, and the check on real machines

**Files:**
- Create: `.github/workflows/windows.yml`, `.github/workflows/macos.yml`, `README.md`

Workflows follow Lunaepix's (`~/sideprojects/Lunaepix/.github/workflows/`): `workflow_dispatch` and `v*` tags only, `permissions: contents: read`, every action pinned to the same full commit hashes Lunaepix uses, `persist-credentials: false`, no `${{ }}` inside a run script, `shell: bash`, `tauri-cli` 2.12.4 via `cargo install --locked`.

- `windows.yml`: job **Test** on `windows-latest` — stage ffmpeg with `--dest "$RUNNER_TEMP/ffmpeg"` and export `VIDEO_OPTIMIZER_TOOLS_DIR` through `$GITHUB_ENV`, then `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --no-fail-fast`, `node --test "tests/ui/*.test.js"` (each later step `if: success() || failure()`). Job **Installer** (needs Test): `cargo tauri build --config src-tauri/tauri.bundle.conf.json`; check that `ffmpeg.exe` and `ffprobe.exe` sit beside the app in the NSIS install folder layout (`target/release/` beside `video-optimizer-app.exe` after the build); upload `target/release/bundle/nsis/*.exe` as artifact `video-optimizer-windows-installer`.
- `macos.yml`: the same Test job on `macos-latest` (Apple Silicon); two Installer jobs, `macos-latest` (artifact `video-optimizer-macos-arm64-dmg`) and `macos-15-intel` (artifact `video-optimizer-macos-intel-dmg`). Each checks, before upload, in the built app and again inside the mounted `.dmg` (Lunaepix's `check_app` step, with `ffmpeg` and `ffprobe` in place of the worker): both programs are in `Contents/MacOS` and executable, `codesign --verify --deep --strict` passes, `Signature=adhoc`, the seal lists both programs, `lipo -archs` of each program is `arm64` on the first job and `x86_64` on the second, and `vtool -show-build` of each gives a `minos` of 11.0 or lower. If `minos` is higher, the job fails and the implementer reports it: `minimumSystemVersion` then has to rise, which is the user's call.
- `README.md`: what it does, the build and test commands (`cargo test --workspace`, `node --test "tests/ui/*.test.js"`, `cargo tauri dev`, `cargo tauri build --config src-tauri/tauri.bundle.conf.json`), the CLI usage, where the CI downloads are, "Open Anyway" on macOS and SmartScreen's "Run anyway" on Windows (as Lunaepix's README), that the ffmpeg builds are pinned in `ffmpeg/builds.toml` and that Intel builds stop at the source in January 2027.

- [ ] **Step 1:** Write the workflows and README. Lunaepix guards its workflows with `src-tauri/tests/workflow.rs`; copy that test and adapt its expectations (pinned hashes, read-only token, no PR trigger, no `${{ }}` in `run:`), with its positive and negative fixture cases.
- [ ] **Step 2: RED** for the copied workflow test before the workflows exist, then GREEN after.
- [ ] **Step 3: Commit** — `Add CI builds and the README`.
- [ ] **Step 4: Hand over for the real-machine check.** The repo has no GitHub remote yet. Stop and ask the user to create the GitHub repository and push the branch (that publishes code, so it is the user's step), then run both workflows by hand. The user then checks, and the report lists as open until done: on an Apple Silicon Mac and an Intel Mac, the `.dmg` opens after "Open Anyway", a video loads, the before/after plays (this is the first proof that WKWebView plays video over the `media` scheme), and a Save writes `optimized-….mp4`; on Windows the same after SmartScreen's "Run anyway"; and a ProRes `.mov` shows `Original (converted for display)`.

---

## Self-review (done while writing)

- **Spec coverage:** settings (T1), never scale / pad / faststart (T4), probe and refusals and 10 GiB (T3), sample and figures and PSNR (T6), save naming / never larger / never overwrite / cancel (T7), CLI (T8), one video / drop / choose other (T9), media playback (T10), slider / views / divider / sync / zoom / fill / hide settings / status (T11, T12), bundled ffmpeg (T13), distribution and real-Mac checks (T13, T14). Every row of the spec's test table has its positive and negative test in the task named. The spec's edges: 1 s and under-10 s clips (T6), no audio (T3, T7), 4K 60 fps (T12's manual run uses the 4K showreel; a generated 3840x2160 60 fps 2 s clip in T7's `keeps_the_size`), spaces and non-ASCII names (T3, T7), Windows backslashes (T2), column hidden during a Save (T12), narrowest window (T12).
- **Uncertain until run:** whether WKWebView plays `<video>` over a custom scheme on macOS 11 with these Range answers (T14 proves it on a real Mac); the minimum macOS of Martin Riedl's builds (T14 checks `minos`); test counts.

## Rules check

Checked:  `~/.claude/CLAUDE.md`, `rules/security.md`, `rules/unit-testing.md`, `rules/vanilla-js.md`, `rules/spec-requirements.md`, `rules/processes.md`, `rules/devsync.md`, `css-conventions.md`, `a11y-rules.md` (`smarty.md` and `cms-defines-fields.md` do not apply: no Smarty, no CMS)
Changed:  added the Handoff with the user's lines verbatim; positive and negative tests in every task, a RED step before every implementation, and the negative tests the checklist demands (failure branches for ffmpeg exit, cancel, unwritable folder, checksum mismatch; "the check can fail" fixtures for the innerHTML, capability and CSP guards); paths go through `file:` and `std::process::Command` with no shell (security, banned idioms); the window gets no path and only `textContent` (vanilla-js); input bounds (10 GiB, CRF range, key digits, Range cap) stated; BBEM class names replace the mock-up's `--` names; a11y: named range inputs, `aria-valuetext`, scroll region with `tabindex="0"`, `inert` while filled, focus return, reduced motion, 16/14 px text, APCA; `## Layout` section with the binding mock-up path; the event payloads written out as real JSON with what the shapes share; the test cases at the edges (CRF 20 and 36, odd sizes, no duration, no audio, rotated, cover art); the GitHub push left to the user; no `devsync` or Grunt step, with the reason.
