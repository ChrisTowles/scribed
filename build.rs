//! Build script. Sets an `$ORIGIN`-relative rpath so the binary can find
//! `libsherpa-onnx-c-api.so` (shipped by `sherpa-rs-sys` with the
//! `download-binaries` feature) without needing `LD_LIBRARY_PATH` set.

fn main() {
    if std::env::var_os("CARGO_FEATURE_ASR").is_some() {
        let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
        match target_os.as_str() {
            "linux" => {
                println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN");
                println!("cargo:rustc-link-arg=-Wl,-rpath,$ORIGIN/../lib");
            }
            "macos" => {
                println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path");
                println!("cargo:rustc-link-arg=-Wl,-rpath,@executable_path/../lib");
            }
            _ => {}
        }
    }
}
