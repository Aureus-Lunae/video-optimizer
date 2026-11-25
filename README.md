# Video optimizer

Video optimizer makes one video smaller. It re-encodes the video to an H.264
MP4 at a quality you choose, shows a 10-second before and after so you can see
what the choice does, and writes the result next to the original. It never
touches or overwrites the original.

For internal use only. It is not built for distribution outside the
organisation (see "ffmpeg" below for why that matters).

There are two ways to use it, built from the same code:

- **The window app** (Tauri), described under "The window".
- **The command line**, `video-optimizer`, described under "The command line".

## What it does

- **Output** is always an H.264 MP4 (`libx264`, preset `slow`, `yuv420p`, fast
  start), at the input's own size. There is no resolution setting and nothing
  is ever scaled. A video with an odd width or height gets one pixel of
  padding to the next even size; an even one gets no filter at all.
- **Quality** is the CRF, a whole number from 20 to 36 (28 to start with). A
  lower number gives a bigger file and a smaller change; a higher number gives
  a smaller file and a bigger change. Anything else is refused.
- **Audio** is kept (AAC, 128 kbit/s) or removed.
- **One video at a time.** There is no batch mode and no queue.
- **The sample** is the first 10 seconds of the video (all of it when it is
  shorter). The window shows it before and after, and the figures for the
  whole file are estimated from it.
- **The file name** is `optimized-<name>.mp4`, next to the original or in a
  folder you choose. When that name is taken, the result is
  `optimized-<name>-1.mp4`, then `-2`, and so on. Nothing is overwritten.
- **Never larger.** When the result is not smaller than the original, the
  original's bytes are copied under the same kind of name and the status says
  `original kept: result was not smaller`. The copy keeps the original's
  extension (`optimized-<name>.mov` for a `.mov`).
- **A cancelled or failed encode leaves no partial file.**
- **Input limit:** 10 GiB.

## The window

The window opens with CRF 28, audio kept, "Next to the original" and the
settings column open, in the Before / after view. Nothing is remembered
between launches.

1. Drop one video on the window, or press "Choose video". Dropping several
   files uses none and says `One video at a time`.
2. The picture shows the before and after next to each other, split by a
   divider, or one at a time at full size. Every number and setting is in the
   column on the right, none below the picture; "Hide settings" folds it away.
3. Move the **Quality** slider (20 to 36) and choose **Audio**. A new sample
   is made a moment after the slider stops moving.
4. Press **Save**. While it runs, the settings are disabled and Cancel stops
   it.

A video the window cannot play (anything but H.264 in `yuv420p`, a ProRes
`.mov` for example) gets a playable copy of its first 10 seconds for the
"before", and the picture says `Original (converted for display)`. The copy
is only for display; the result is always made from the original.

## The command line

```
video-optimizer [--crf <20-36>] [--no-audio] [--out <folder>] [--] <file>
```

- `--crf <20-36>` is the quality, a whole number (28 when left out).
- `--no-audio` removes the sound.
- `--out <folder>` is an existing folder for the result, instead of the
  original's own folder.
- `--` ends the options, so a file named `-x.mp4` can be given after it.

```
video-optimizer promo.mov                      # optimized-promo.mp4, CRF 28
video-optimizer --crf 32 --no-audio promo.mov  # a smaller one, without sound
video-optimizer --out small -- promo.mov       # into the folder ./small
```

It prints one line to stdout:

```
optimized-promo.mp4: 68.3 MB -> 5.8 MB (91% smaller)
optimized-promo.mov: original kept: result was not smaller
```

The percent of the encode goes to stderr as `\r42%`, and only when stderr is
a terminal. The exit code is 0 when it is done (also when the original was
kept), 1 when the video failed (then no file is left behind), 2 when the
arguments were wrong, and 130 when Ctrl-C stopped it.

**Ctrl-C** stops the encode: the program stops ffmpeg, deletes the hidden
`.optimized-*.part` file it was writing, prints `cancelled` to stderr and
nothing to stdout, so the folder is as it was. This holds on Windows too,
where ffmpeg runs without a console and does not get the Ctrl-C itself. A
Ctrl-C that comes after the file is saved changes nothing.

Build it with `cargo build --release -p video-optimizer-cli`; the program is
`target/release/video-optimizer`. It needs `ffmpeg` and `ffprobe`: next to the
program, in the folder named by `VIDEO_OPTIMIZER_TOOLS_DIR`, or on the path
(in that order).

## ffmpeg

ffmpeg 9.1 is bundled with the app. The builds are pinned in
[`ffmpeg/builds.toml`](ffmpeg/builds.toml), each by its release address and its
SHA-256, which the build checks before it unpacks anything:

- macOS: Martin Riedl's static release builds (<https://ffmpeg.martin-riedl.de>).
- Windows x64: Gyan's "essentials" build, release 9.1 of `GyanD/codexffmpeg`.

**Intel Macs.** Martin Riedl stops making macOS Intel release builds in
January 2027. The pinned 9.1 Intel build keeps working, but a later ffmpeg
upgrade for Intel Macs needs another source.

**Licence.** These builds contain x264, which is GPL. That is fine while the
app is for internal use only. If it is ever shared outside the organisation,
the licence question comes back first.

## Building and testing

You need Rust 1.90 or newer (<https://rustup.rs>) and Node 22 (the window
script tests need Node 21 or later). On Linux, Tauri also needs a few system
packages; see <https://tauri.app/start/prerequisites/>. There is no Linux
build of the app.

```
cargo install tauri-cli --locked --version 2.12.4               # once
cargo tauri dev                                                 # runs the window from the source
cargo tauri build --config src-tauri/tauri.bundle.conf.json     # bundles end up in target/release/bundle/
cargo test --workspace                                          # the Rust tests
node --test "tests/ui/*.test.js"                                # the window script tests
```

The `--config` file adds ffmpeg and ffprobe to the bundle: it first runs
`cargo run --locked -p video-optimizer-xtask -- ffmpeg`, which downloads the
pinned builds into `ffmpeg-cache/`, checks them and copies them to
`src-tauri/binaries/`, and then makes them sidecars that Tauri installs beside
the app. Without it the app has no ffmpeg of its own.

**The tests use a real ffmpeg.** They find it through
`VIDEO_OPTIMIZER_TOOLS_DIR` or on the path. To test with the pinned build:

```
cargo run --locked -p video-optimizer-xtask -- ffmpeg --dest /tmp/ffmpeg
VIDEO_OPTIMIZER_TOOLS_DIR=/tmp/ffmpeg cargo test --workspace
```

The tests that cancel a running encode (`crates/core/tests/run.rs`, and the
Ctrl-C tests in `crates/cli/tests/cli.rs`) find ffmpeg's process with `pgrep`
(procps), so `pgrep` has to be installed. macOS has it, and the Mac runners of
the CI use it; on Linux install `procps`. These tests are compiled for Unix
only, so Windows needs no `pgrep`.

## Getting the installers

The installers are built on GitHub Actions; nothing here builds them for
release. Open the repository's Actions tab, choose a workflow and press "Run
workflow". Pushing a tag that starts with `v` (for example `v0.1.0`) starts
both too. Each run checks the whole workspace first; when it is green,
download the artifact from the run's page. GitHub delivers each as a zip.

| Workflow | Artifact | Holds |
|---|---|---|
| Windows build (`.github/workflows/windows.yml`) | `video-optimizer-windows-installer` | an NSIS installer (`.exe`), Windows 64-bit |
| macOS build (`.github/workflows/macos.yml`) | `video-optimizer-macos-arm64-dmg` | a `.dmg` for Apple Silicon Macs (M1 or later) |
| macOS build (`.github/workflows/macos.yml`) | `video-optimizer-macos-intel-dmg` | a `.dmg` for Intel Macs |

Both need macOS 11 or later. Neither is signed with a certificate, so the
first launch asks you to confirm it.

### macOS: "Open Anyway"

The app is **not signed with an Apple developer certificate** (only signed ad
hoc), so macOS blocks the first launch. To get past it, once:

1. Open the `.dmg` and drag Video optimizer into Applications.
2. Open Video optimizer from Applications, then close the warning that appears.
3. Open System Settings > Privacy & Security and scroll down to Security.
4. Next to the message about Video optimizer, choose "Open Anyway" and
   confirm.

After that it opens normally. Since macOS Sequoia, Right-click > Open no longer
gets past the block; use "Open Anyway" in System Settings.

### Windows: SmartScreen

The installer is **not signed**, so Windows SmartScreen warns when you start
it: choose "More info", then "Run anyway".
