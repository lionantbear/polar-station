fn main() {
    let manifest = std::path::PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    // Both native frontends already link libpolar_tmux. Embed the common C
    // admission code in that archive, rather than maintaining a UI copy.
    if target == "macos" || target == "linux" {
        let display = manifest.join("../../apps/polar-tmux-shared/display");
        println!("cargo:rerun-if-changed={}", display.join("station_display.c").display());
        println!("cargo:rerun-if-changed={}", display.join("station_display.h").display());
        cc::Build::new()
            .file(display.join("station_display.c"))
            .include(&display)
            .flag_if_supported("-std=c11")
            .flag_if_supported("-pthread")
            .warnings(true)
            .compile("polar_station_display");
        println!("cargo:rustc-link-lib=pthread");
    }
    if target == "linux" {
        let audio = manifest.join("../../apps/polar-tmux-shared/dictate/station_dictate_audio_linux.c");
        println!("cargo:rerun-if-changed={}", audio.display());
        cc::Build::new().file(audio).include(manifest.join("include"))
            .flag_if_supported("-std=c11").warnings(true).compile("polar_station_dictate_audio");
        println!("cargo:rustc-link-lib=dl");
    }
    if target != "macos" { return; }
    let info = manifest.join("../../automation/polar_station/host-macos-info.plist");
    println!("cargo:rerun-if-changed={}", info.display());
    println!(
        "cargo:rustc-link-arg-bin=polar-tmux=-Wl,-sectcreate,__TEXT,__info_plist,{}",
        info.display()
    );
}
