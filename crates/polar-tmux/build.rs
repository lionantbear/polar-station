fn main() {
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target != "macos" {
        return;
    }
    let info = manifest.join("host-macos-info.plist");
    println!("cargo:rerun-if-changed={}", info.display());
    println!(
        "cargo:rustc-link-arg-bin=polar-tmux=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        info.display()
    );
}
