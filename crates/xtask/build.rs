// The triple this helper is built for: the machine it runs on, because
// `cargo run` builds it without `--target`. The helper uses it when
// `TAURI_ENV_TARGET_TRIPLE` is not set, so a plain `cargo run` on a Mac or a
// Windows PC stages the programs for that machine.
fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=VIDEO_OPTIMIZER_HOST_TRIPLE={target}");
}
