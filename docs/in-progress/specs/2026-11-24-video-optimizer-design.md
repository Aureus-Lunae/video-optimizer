# Video optimizer — design

**Owner:** Aureus-Lunae · **Started:** 2026-11-25

## Handoff

Decided in conversation (2026-11-24). The user's words are quoted as they were said.

- **Origin:** the user compressed five Premiere Pro exports (`~/Videos/postads test/`) with ffmpeg and asked: "if I want to make something for this similar to lunaepix?"
- **CRF 28 is the starting point:** "CRF28. Lets see the actual differences." Then "Well, try a CRF 30 on them." Measured results are in "Evidence".
- **Separate app, not inside Lunaepix:** answered "Separate app (Recommended)". Repo `~/sideprojects/video-optimizer`, same architecture as Lunaepix.
- **ffmpeg is bundled:** answered "Bundle it (Recommended)". Ruled out: "Require it installed", "Detect, else offer download".
- **Internal use only:** "It is for internal use only." Not distributed outside the organisation, so the x264 GPL obligations do not apply and no licence screen is built. If it is ever shared outside, the licence question comes back first.
- **Platforms:** "Internal means mac", then "And windows." So macOS and Windows; no Linux build.
- **Not everyone is on Apple Silicon:** asked whether everyone is on an Apple Silicon Mac, the user answered "No". So Intel Macs are built too.
- **Right column starts open:** "Column starts open".
- **Slider range:** "20 to 36". CRF 20 to 36, set by the user (HandBrake's own range is 0 to 51).
- **Quality is a slider, like HandBrake:** "the CRF can be a slider, like in handbreak". This replaces the earlier "No quality slider, just the encoding CRF28, CRF26, CRF30 and CRF23" (four fixed choices), which is no longer valid. The slider shows the CRF number as it moves.
- **No downscaling, ever:** asked whether the first version offers a 1080p choice: "Never". There is no resolution setting; the output keeps the input's size.
- **Single video only, no Batch:** "BATCH removed dammit. I asked for single video only." There is no Batch mode, no queue, no file list and no mode switch. One video at a time.
- **Name:** "Name also removed. Just call it video optimizer." The app is called "Video optimizer". No other name is used anywhere, including the command line and the repo.
- **Before/after at full size:** "Single video only, you show it next to eachother,but is it possible to also do a before after so we can see the full sized video?" Both views exist.
- **Layout like Lunaepix:** "Lunaepic also has all data in a tab that comes in from the right when selected, not below the image." Every piece of data and every setting is in a column on the right, none below the picture.
- **Layout:** "HTML mockup". The mock-up is [2026-11-24-video-optimizer-mockup.html](2026-11-24-video-optimizer-mockup.html). The user approved it ("Ok, that looks good"), so it is binding.
- **Spec approved:** "Lets start the work on this." (2026-11-25)
- **Plan approved:** "yes" (2026-11-25), subagent-driven. The plan is `docs/in-progress/plans/2026-11-25-video-optimizer.md`; it settles the ffmpeg builds (9.1: Martin Riedl for macOS, Gyan "essentials" for Windows).
- **An original the window cannot play:** asked what the window does with a file it cannot play (Apple ProRes, or HEVC on some Windows PCs), the user answered "Show a playable copy (Recommended)". Only for such a file, the "before" is a near-lossless H.264 copy of the first 10 seconds (CRF 10), labelled "Original (converted for display)". Ruled out: "Show a message instead", "Refuse the file".
- **My calls, not decisions:** the default choice is CRF 28; the slider moves in steps of 1; audio keep/remove stays as a setting; the right column folds away with "Hide settings" as in Lunaepix (my reading of "comes in from the right when selected").
- Put off: AV1/WebM output, HEVC output, a brew-style install.

## Problem

Premiere Pro's H.264 export has no CRF (constant quality) setting, only bitrate. Its exports are 15 to 21 Mbps, far more than the web needs. A 36-second banner is 68 MB. Re-encoding with ffmpeg at CRF 28 makes it 5.8 MB, but that needs a terminal.

## Evidence (ffmpeg, libx264, preset slow, PSNR on the first 10 s)

| File | Original | CRF 23 | CRF 28 | CRF 30 |
|---|---|---|---|---|
| Banner 1328x747 | 68.3 MB | 12.0 MB | 5.8 MB | 4.5 MB |
| Banner 2560x1440 | 68.1 MB | 38.5 MB | 18.3 MB | 13.8 MB |
| Showreel 4K, audio | 96.9 MB | 89.6 MB | 40.8 MB | 30.2 MB |
| Showreel 4K, no audio | 95.5 MB | **107.1 MB** | 49.8 MB | 34.6 MB |

CRF 26 and every other value between the measured ones were not measured; the mock-up's figures for them are estimates. The last row at CRF 23 is larger than its input, so the app must never write a result larger than the input.

## Goals

- Make one video smaller for the web, never touching the original.
- Output is always an H.264 MP4 with `+faststart`, at the input's own size.
- A window app (Tauri) and a command line, built from one `core` crate, like Lunaepix. For macOS (Apple Silicon and Intel) and Windows.

## What changes today / what stays

Nothing exists yet. Lunaepix is not changed.

## Design

### Settings

| Setting | Values | Default |
|---|---|---|
| Quality | a slider, CRF 20 to 36, whole numbers only | CRF 28 |
| Audio | Keep (AAC 128k), Remove | Keep |

Fixed: `-preset slow`, `-pix_fmt yuv420p`, `-movflags +faststart`. The size is never changed. A source with odd dimensions is padded by one pixel to the next even size, never scaled (x264 with yuv420p needs even sizes).

### Behaviour

- The app holds one video. "Choose other" in the right column replaces it. There is no list and no queue.
- A video is chosen by dropping it on the window or with a file chooser. Dropping several files uses none of them and says so ("One video at a time").
- The app encodes the first 10 seconds as a sample at the chosen CRF and shows it against the original. Changing the CRF makes a new sample.
- **Save** encodes the whole video with a progress bar in the status line and a Cancel button; a cancelled encode deletes its partial output.
- Output goes next to the original as `optimized-<name>.mp4`, or into a chosen folder. A name that is taken gets a number (`-1`, `-2`); nothing is overwritten.
- If the result is not smaller than the input, the original bytes are copied and the status says `original kept: result was not smaller`.
- A file with no video stream, or one ffmpeg cannot read, is refused with ffmpeg's reason, and the current video stays.
- Limit: one file at most 10 GiB.

### The picture and its comparison

- Two views, switched above the picture: **Before / after** (the default) and **Side by side**.
- Before / after stacks the original and the sample at full size with a draggable divider that also works with the keyboard (a range input named "Before and after divider"). The divider starts in the middle.
- Side by side shows them next to each other.
- Both videos play, pause and seek together, with one play button and one position bar. The sample is kept in step with the original; a drift above one frame is corrected.
- **Fill window** shows the comparison over the whole window; Escape closes it. **Zoom 100%** shows one video pixel per screen pixel, and dragging moves around the picture.
- Only the 10-second sample is compared, never the full encode.
- The "before" is the original's first 10 seconds, copied without re-encoding. When the window cannot play the original's format, it is a near-lossless H.264 copy (CRF 10) instead, labelled "Original (converted for display)".

### The right column

Everything except the picture lives in one column on the right, like Lunaepix. Nothing is placed below the picture except the play button and position bar, which control the picture.

From top to bottom: "Hide settings" button; then a part that scrolls on its own: **File** (name, size in pixels, frame rate, length, audio) with "Choose other"; **Quality** (the slider); **Audio**; **Save to** with "Choose folder"; **Result for the first 10 seconds** (original, optimized, smaller by, estimated whole file, PSNR); and a bottom part that stays visible with the status line and **Save**.

"Hide settings" folds the column away and a "Show settings" button appears over the top right of the picture. The column slides in and out; with reduced motion set, it does not animate.

### Command line

`video-optimizer <file> --crf 28 --no-audio --out <dir>`. One file. `--crf` accepts a whole number from 20 to 36. Same naming rules as the window.

### Bundled ffmpeg

An `ffmpeg` and `ffprobe` build per target is shipped inside the app (Tauri sidecar): macOS arm64, macOS x86_64 and Windows x64. The exact builds are settled in the plan.

### Distribution (same route as Lunaepix)

See Lunaepix's `2026-11-13-lunaepix-apple-design.md` and its Windows workflow.

- **Windows:** a GitHub Actions job on `windows-latest` builds the installer, kept as a run artifact.
- **Apple Silicon:** a job on `macos-latest` builds an ad-hoc-signed `.dmg` (`signingIdentity: "-"`). Apple Silicon refuses unsigned code as "damaged"; ad hoc turns that into the "Open Anyway" step. No $99 developer fee.
- **Intel Mac:** a second job on an Intel runner builds its own `.dmg` with the x86_64 ffmpeg. A universal build is not used: it needs both ffmpeg builds in one app and nobody asked for it.
- The bundled `ffmpeg` and `ffprobe` are ad-hoc signed as well, or the app cannot start them. The plan checks this on a real Mac of each kind.
- `minimumSystemVersion` 12.0 (was 11.0: the bundled ffmpeg builds need macOS 12, decided 2026-11-25, "Raise to macOS 12 (Recommended)"). The README explains "Open Anyway" and Windows SmartScreen's "Run anyway".

## Layout

The binding layout is the approved mock-up [2026-11-24-video-optimizer-mockup.html](2026-11-24-video-optimizer-mockup.html): the picture fills the window, the column on the right holds all data and settings, and the quality is a slider (CRF 20 to 36, step 1) set to 28 when the app opens, with the CRF number and a short word beside it, and "Better quality, bigger file" / "Smaller file" under its two ends. A launch never remembers the last choice (like Lunaepix, "Start fresh").

## Tests (positive and negative each)

| Behaviour | Positive | Negative |
|---|---|---|
| CRF value | 20, 28 and 36 are accepted | 19, 37, 28.5, 0, 51, -1, "28", NaN and an array are refused (slider, CLI and the encoder each) |
| Slider | moving it changes the CRF, the sample and the result figures | it stops at 20 and 36; the keyboard arrows move one step, Home and End the ends |
| One video only | choosing a second video replaces the first | dropping two files uses neither and shows the message |
| Never larger | a smaller result is written | a larger result keeps the original bytes and the note |
| Never overwrites | `optimized-a.mp4` taken gives `optimized-a-1.mp4` | existing files are byte-identical afterwards |
| Size is kept | output width and height equal the input's (even-sized source) | no scale filter is ever passed to ffmpeg |
| Odd dimensions | 1281x721 is padded to 1282x722 | an even source is not padded |
| Audio | Remove gives no audio stream | Keep gives an audio stream |
| Cancel | partial output is deleted | a finished file is not deleted by a later cancel |
| Unreadable input | a text file renamed `.mp4` is refused with a reason and the current video stays | a valid file is not refused |
| Progress | out_time/duration gives 0 to 100 | a missing duration shows indeterminate, not NaN |
| ffmpeg failure | a non-zero exit shows the failure and removes the partial output | exit 0 shows done |
| Before / after divider | 0 shows only the sample, 100 only the original, 50 half each | -1, 101 and "abc" are clamped or ignored, never breaking the picture |
| Playback in step | play, pause and seek on one move the other | the two never differ by more than one frame after a seek |
| Zoom 100% | one video pixel is one screen pixel | back to fit-to-window shows the whole frame again |
| Hide settings | hiding folds the column and shows "Show settings" | showing again restores the column with the same choices |
| Process arguments | a name with spaces, quotes or non-ASCII is passed as one argument | a name starting with `-` is not read as an option |

Edges: a 1 s clip, a clip shorter than 10 s (the sample is the whole clip), a clip with no audio, a 4K 60 fps clip, a name with spaces and non-ASCII characters, a Windows path with backslashes, the column hidden while a Save runs, and the narrowest window.

## Rules check

Checked:  `security.md` (file paths and process arguments), `unit-testing.md`, `vanilla-js.md`, `a11y-rules.md` (both range inputs named, the slider's value read aloud as "CRF 28", focus visible, reduced motion), `css-conventions.md`, `spec-requirements.md`
Changed:  Batch and the name removed; layout moved to a right-hand column; quality is now a slider; edges and negative tests added; file paths go to ffmpeg as separate arguments, never through a shell, and never starting with `-`.

## Open questions for the user

None. The user approved the mock-up ("Ok, that looks good") and every question is answered.
