# ffprobe fixtures

Each file is the output of `ffprobe -v error -print_format json -show_format
-show_streams` from **ffprobe 8.1.3**, run on a clip made with ffmpeg 8.1.3, and
then trimmed to the fields `probe.rs` reads (the JSON has the same shape in
ffprobe 9.1). The tests change single fields of a fixture where they need a
variant (a rotation, a duration, a frame rate).

| File | What it is |
|---|---|
| `full.json` | H.264 video with AAC audio. The structure is captured; the numbers are set by hand to those of a real Premiere export: height 747 (ffmpeg's encoder cannot write an odd height), audio bit rate 317000. |
| `no_audio.json` | The same video without the audio stream. |
| `rotated.json` | 1920x1080 H.264 with a display matrix of -90 degrees in `side_data_list`, and audio. Captured as is. |
| `cover_only.json` | An `.m4a`: an audio stream and an `mjpeg` stream with `attached_pic: 1`. Captured as is. |
| `cover_first.json` | An `mjpeg` cover (`attached_pic: 1`) as stream 0, then the H.264 video as stream 1. ffprobe lists an mp4's cover last, so the captured streams were put in the order an mp3 or a tagged file gives them; the audio stream was left out. |
