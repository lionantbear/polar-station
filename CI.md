# Continuous integration

`Station source build` runs on main pushes, pull requests and manual dispatch.
It uses Linux (Ubuntu 24.04) and macOS 15 GitHub-hosted runners, with the
repository's pinned Rust toolchain and Cargo.lock. Both jobs execute
`cargo build --workspace --locked` (actual compilation/linking, not just
`cargo check`). The macOS job additionally executes the Swift package build
against `target/debug/libpolar_tmux.a`.

A green workflow means these source components compile. It does **not** mean
the complete native application or release package has been built or tested.
SwiftPM contains library targets, not the full Ghostty application.

## Full native-app build gaps

The standalone Rust runtime builder is now included, and Linux graphics builds
invoke Cargo directly instead of depending on an absent monorepo justfile. These
are build-entry repairs, not evidence of a successful full Linux GUI build.
The Linux installer still requires icon inputs not included in the export and
a relocatable end-user package. See RELEASE_PLAN.md. Legacy-name cleanup remains
a separate task.

Both full GUI builds require the exact documented Ghostty/Godot revisions,
Zig, SCons, editor and Graphviz resources. macOS packaging uses ad-hoc signing;
this CI does not request Apple certificates or notarization. Companion remains
disabled by the public profile. CI neither publishes binaries nor changes the
source-only release's licensing or legal-release gates.

No secrets are required. Jobs have read-only repository permissions, a
60-minute timeout and two Cargo build workers. New runs supersede obsolete
runs on the same branch. Initial build results must be read from Actions;
adding this workflow is not itself evidence of a successful build.

## Native macOS 26 validation

`macOS 26 native app validation` is a separate manual workflow (also runs when
its workflow file changes on main). It attempts the real embedded Godot,
Ghostty/native app and ad-hoc signed DMG pipeline on Apple Silicon macOS 26.
Ghostty/Godot sources are commit-pinned; downloaded Zig and Godot editor
archives are SHA-256 checked. It deliberately does not upload binaries or
create a Release. Build success is distinct from fresh-machine install/launch,
graphics/TCC testing and exact bundled-license clearance. Homebrew Graphviz
versions are recorded but not yet locked as reproducible release inputs.

### macOS 26 toolchain compatibility

The initial native run on macOS 26 completed Godot/graphics and the Rust release
build, then failed while Zig 0.15.2 linked its build runner against Xcode 26.6
(`__availability_version_check` and libc symbols unresolved). Ghostty upstream
tracks this as [issue 11991](https://github.com/ghostty-org/ghostty/issues/11991).
The native workflow now selects the already-installed Xcode 26.3 using only
`DEVELOPER_DIR`, keeps that selection for Xcode invocation, and tests a minimal
Zig build runner before expensive compilation. The OS remains macOS 26; no
compiler pin, Apple security setting, or SDK content is modified. This fix must
be confirmed by the new native run, not assumed from the previous source CI.

## Private trial retention

The current exported-source native pipeline compiles the app and verifies its
signatures, installed-copy launch and real terminal child before retaining a
private draft. Run 43084976157 passed those product checks; retention stopped
after an anonymous-denial request returned HTTP 403. Its expensive DMG and two
native source captures were retained, but the remaining evidence was not.

Retention must distinguish explicit API rate limits from a proven private 404.
GET-only retries may wait for GitHub's advertised rate-limit reset within a
65-minute total budget; every successful privacy check still requires 404.
An unexplained 403, an excessive wait or any public response remains a failure.
Upload the complete explicit file selection and verify its authenticated bytes
before checking each asset anonymously, so a delayed anonymous check does not
discard the remaining build evidence. Never retry uncertain writes, replace
assets, publish the raw draft, or weaken final publication review.
