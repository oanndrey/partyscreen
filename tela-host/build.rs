fn main() {
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // screencapturekit usa uma ponte em Swift; o runtime fica em /usr/lib/swift no macOS
    if os == "macos" {
        println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    }
    // Ícone e informações do .exe (aparecem no Explorer e na barra de tarefas)
    if os == "windows" {
        println!("cargo:rerun-if-changed=assets/icon.ico");
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico")
            .set("ProductName", "Tela P2P")
            .set("FileDescription", "Tela P2P")
            .set("CompanyName", "Tela P2P")
            .set("LegalCopyright", "Tela P2P");
        res.compile().expect("falha ao embutir o ícone");
    }
}
