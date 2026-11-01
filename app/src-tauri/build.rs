fn main() {
    // screencapturekit usa uma ponte em Swift; o runtime fica em /usr/lib/swift no macOS
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    tauri_build::build()
}
