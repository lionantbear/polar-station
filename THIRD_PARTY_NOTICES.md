# Third-party source provenance — NOT a completed license audit

## Ghostty

- Repository: https://github.com/ghostty-org/ghostty.git
- Tag: v1.3.1
- Revision: 332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28
- Ghostty is MIT-licensed; its official full LICENSE must accompany
  the fetched upstream source and redistributed binaries.

## Other reviewed source-level license claims

- **ghostty-macos-and-linux**: `MIT`; pinned/input version: `v1.3.1 / 332b2aefc6e72d363aa93ab6ecfc86eeeeb5ed28`.
  Official evidence: https://github.com/ghostty-org/ghostty/blob/main/LICENSE
  License text in this source tree: licenses/pinned/ghostty/LICENSE.
  Additional conditions: Exact pinned LICENSE fetched and hashed; recursively linked dependencies and binary notices remain to verify.
- **godot-engine**: `MIT`; pinned/input version: `ed1daf0bf001b61586d9930840f2f1394092c079 / editor 4.7.2`.
  Official evidence: https://github.com/godotengine/godot/blob/master/LICENSE.txt
  License text in this source tree: apps/polar-graphics-native/licenses/Godot-LICENSE.txt.
  Additional conditions: Bundled LICENSE and COPYRIGHT match byte-for-byte the exact pinned Git source. Actual binary dependency coverage remains unverified.
- **godot-rust-gdext**: `MPL-2.0`; pinned/input version: `0.4.5`.
  Official evidence: https://github.com/godot-rust/gdext/tree/v0.4.5
  License text in this source tree: crates/polar-force-layout-core/upstream/LICENSE-godot-rust-MPL-2.0.txt.
  Additional conditions: Unmodified upstream adapter according to local NOTICE. File-level copyleft obligations, including modifications to gdext if any, require review.
- **graphviz**: `EPL-2.0 root license for observed 16.1.0; mixed-license source notices require review`; pinned/input version: `16.1.0 observed in native CI; Homebrew build input remains unlocked`.
  Official evidence: https://graphviz.org/license/
  License text in this source tree: licenses/native-source-texts/0becf16567beb77fa252b7664631dd177c8f9a1889e48995b45379c7130e5303.txt.
  Additional conditions: Exact 16.1.0 archive checksum and root EPL-2.0 text are retained in licenses/native-source-evidence.json. Captured Homebrew metadata incorrectly says EPL-1.0 for this source archive. Additional third-party notices, generated-parser exceptions, copied dylibs and exact bottle composition remain unapproved; source-text presence does not satisfy binary distribution obligations.
- **erofs-utils**: `GPL-2.0-or-later OR MIT (liberofs predominantly); other vendored files differ`; pinned/input version: `native/upstream.lock.json in polar-cas-fs-sys; pinned vendor copy`.
  Official evidence: https://github.com/erofs/erofs-utils
  License text in this source tree: crates/polar-cas-fs-sys/native/vendor/erofs-utils/COPYING.
  Additional conditions: Local COPYING says lib/include largely dual GPL-2.0+ OR MIT. Some other files are GPL-only, BSD-2-Clause OR GPL, or Unlicense. File-level SPDX scan and exact compiled-file selection required; retain license files for every redistributed source.
- **d3-force-3d**: `MIT`; pinned/input version: `263b3bfc37578e541a7d7e59dbb32d026d66fa38`.
  Official evidence: https://github.com/vasturiano/d3-force-3d/blob/master/LICENSE
  License text in this source tree: crates/polar-force-layout-core/upstream/d3-force-3d/LICENSE.
  Additional conditions: Native Rust force-layout code adapts algorithmic behavior; preserve upstream attribution in source and binary notices.
- **d3-octree**: `MIT`; pinned/input version: `28beff983b94757c90b003c51c2c8a053ddd63be`.
  Official evidence: https://github.com/vasturiano/d3-octree
  License text in this source tree: crates/polar-force-layout-core/upstream/d3-octree/LICENSE.
  Additional conditions: 3D octree implementation is an adaptation; native LICENSE and NOTICE retained.
- **nucleo-git**: `MPL-2.0`; pinned/input version: `4253de9faabb4e5c6d81d946a5e35a90f87347ee`.
  Official evidence: https://github.com/helix-editor/nucleo
  License text in this source tree: licenses/pinned/nucleo/LICENSE.
  Additional conditions: Exact pinned MPL-2.0 text fetched and hashed; source availability and any file modifications remain to verify.

## Cargo Git source attribution

- `https://codeberg.org/Lonami/grammers.git@4ca2263e5678d2e85414aa8c2f469c1341d3a143`: MIT OR Apache-2.0.
  Source/evidence: https://codeberg.org/Lonami/grammers; exact commit license: exact pinned root license text fetched and hashed; package-level and binary obligations remain to verify.
- `https://github.com/helix-editor/nucleo.git@4253de9faabb4e5c6d81d946a5e35a90f87347ee`: MPL-2.0.
  Source/evidence: https://github.com/helix-editor/nucleo/blob/master/LICENSE; exact commit license: exact pinned root license text fetched and hashed; package-level and binary obligations remain to verify.
- `https://github.com/leoricshcn/codex.git@7eea594e1264e37e08f029633f2f476f03fbda6c`: Upstream Codex Apache-2.0; fork's pinned source to verify.
  Source/evidence: https://github.com/openai/codex; exact commit license: exact pinned root license text fetched and hashed; package-level and binary obligations remain to verify.
- `https://github.com/microsoft/mxc@6cd3d58f05d3447e67109cfb75e042803b843ca4`: MIT.
  Source/evidence: https://github.com/microsoft/mxc; exact commit license: exact pinned root license text fetched and hashed; package-level and binary obligations remain to verify.
- `https://github.com/modelcontextprotocol/rust-sdk@4048ac40e958fb1a7d614617a2ae8d26666ee2fa`: Hybrid Apache-2.0/MIT/CC-BY-4.0 history; inspect exact revision.
  Source/evidence: https://github.com/modelcontextprotocol/rust-sdk/blob/main/LICENSE; exact commit license: exact pinned root license text fetched and hashed; package-level and binary obligations remain to verify.
- `https://github.com/openai-oss-forks/tokio-tungstenite@132f5b39c862e3a970f731d709608b3e6276d5f6`: Upstream MIT; fork exact source to verify.
  Source/evidence: https://github.com/snapview/tokio-tungstenite; exact commit license: exact pinned root license text fetched and hashed; package-level and binary obligations remain to verify.
- `https://github.com/openai-oss-forks/tungstenite-rs@9200079d3b54a1ff51072e24d81fd354f085156f`: Upstream MIT OR Apache-2.0; fork exact source to verify.
  Source/evidence: https://github.com/snapview/tungstenite-rs; exact commit license: exact pinned root license text fetched and hashed; package-level and binary obligations remain to verify.

**Provisional inventory only. The full resolved Cargo license report,
exact upstream/Godot/Graphviz notices and binary-level dependency
inventory MUST be generated and reviewed before distribution.**
