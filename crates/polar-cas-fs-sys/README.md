# polar-cas-fs-sys

Linux C FFI for PolarCAS positional I/O and EROFS. This crate includes the exact
upstream C source under `native/vendor/erofs-utils`. No source download, parent
repository checkout, or Python interpreter is used by the Cargo build.

## Native build requirements

A native Linux C compiler, CMake >= 3.18, GNU make, autoconf, automake, libtool,
patch, pkg-config, and liblz4 development headers/library. These are build tools,
not extra Polar services. Cross compilation is currently rejected explicitly.

`build.rs` calls CMake in Cargo's OUT_DIR. CMake checks the bundled source file
set and each SHA-256, applies the checked-in patch to a private copy, configures
upstream without remote/FUSE backends, and builds static archives. Source files
in the package are never modified. Missing tools or mismatched hashes fail the
build; there is no fallback to prebuilt or unchecked libraries.

```
cargo test -p polar-cas-fs-sys --features erofs
```

The core's small native compatibility checks can also run without Cargo:

```
cmake -S native -B /tmp/pcfs-build -DPOLAR_EROFS=ON -DBUILD_TESTING=ON
cmake --build /tmp/pcfs-build --parallel 2
ctest --test-dir /tmp/pcfs-build --output-on-failure
```

These C tests do not replace Rust type-checking, integration tests, or kernel
mount acceptance. No directory ingest, Catalog, or release status is implied.

Upstream copyright notices and licenses remain in native/vendor/erofs-utils.
This package selects the MIT alternative for the bundled dual-licensed library
sources. The tool executables in the upstream tree are not built by this crate.
