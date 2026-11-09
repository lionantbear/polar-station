//! Offline native build. Cargo's build script is Rust; no Python is invoked.
use std::{env, path::PathBuf, process::Command};

fn run(command: &mut Command) {
    let description = format!("{command:?}");
    let status = command
        .status()
        .unwrap_or_else(|error| panic!("cannot start {description}: {error}"));
    assert!(
        status.success(),
        "native build failed: {description}: {status}"
    );
}

fn main() {
    println!("cargo:rerun-if-changed=native");
    println!("cargo:rerun-if-env-changed=CC");
    println!("cargo:rerun-if-env-changed=AR");
    println!("cargo:rerun-if-env-changed=CFLAGS");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    assert_eq!(
        env::var("HOST").expect("HOST"),
        env::var("TARGET").expect("TARGET"),
        "native cross compilation is not enabled; use a native Linux builder"
    );
    let source =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir")).join("native");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR")).join("pcfs-native");
    let erofs = env::var_os("CARGO_FEATURE_EROFS").is_some();
    let mut configure = Command::new("cmake");
    configure
        .arg("-S")
        .arg(source)
        .arg("-B")
        .arg(&out)
        .arg("-DCMAKE_BUILD_TYPE=Release")
        .arg("-DBUILD_TESTING=OFF")
        .arg(if erofs {
            "-DPOLAR_EROFS=ON"
        } else {
            "-DPOLAR_EROFS=OFF"
        });
    if let Some(cc) = env::var_os("CC") {
        configure.arg(format!("-DCMAKE_C_COMPILER={}", cc.to_string_lossy()));
    }
    if let Some(ar) = env::var_os("AR") {
        configure.arg(format!("-DCMAKE_AR={}", ar.to_string_lossy()));
    }
    configure.arg(format!(
        "-DCMAKE_C_FLAGS={}",
        env::var("CFLAGS").unwrap_or_default()
    ));
    run(&mut configure);
    run(Command::new("cmake")
        .arg("--build")
        .arg(&out)
        .arg("--parallel")
        .arg(env::var("NUM_JOBS").unwrap_or_else(|_| "2".into())));
    println!(
        "cargo:rustc-link-search=native={}",
        out.join("lib").display()
    );
    println!("cargo:rustc-link-lib=static=polar_cas_fs_io_v1");
    if erofs {
        println!("cargo:rustc-link-lib=static=erofs");
        println!("cargo:rustc-link-lib=lz4");
    }
    println!("cargo:rustc-link-lib=pthread");
}
