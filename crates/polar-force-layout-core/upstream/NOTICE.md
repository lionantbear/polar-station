# Force layout source provenance and adaptation boundary

This directory is a **development-only oracle**, not a JavaScript runtime shipped
inside the Godot application. `PROVENANCE.json` records the upstream commit and Git
blob identity of every vendored source. The sole byte normalization is a final LF
added to `d3-octree/src/index.js` by patch transport; its original source lacks
that LF. `oracle.mjs` reconstructs and verifies that exact original byte sequence
and verifies all other files verbatim before comparing the native executable
against the upstream functions. No upstream algorithm statements are modified.

## Source-derived native code

* `d3-force-3d` commit `263b3bfc37578e541a7d7e59dbb32d026d66fa38`:
  `simulation.js`, `link.js`, `manyBody.js`, `center.js`, `lcg.js`, `jiggle.js`,
  `constant.js`. Copyright (c) 2017 Vasco Asturiano, MIT; full license retained in
  `d3-force-3d/LICENSE`.
* `d3-octree` commit `28beff983b94757c90b003c51c2c8a053ddd63be`:
  `add.js`, `cover.js`, `visit.js`, `visitAfter.js` provide the insertion,
  coincident-point ordering, bounds and traversal behavior adapted by native
  `src/octree.rs`. Copyright (c) 2017 Vasco Asturiano, MIT; full license retained in
  `d3-octree/LICENSE`. The other vendored octree files complete the unchanged JS
  reference module, but are not independently reimplemented in native code.

The Rust implementation is an **adaptation**, not unchanged reuse. It preserves
the numerical update order and supports the 3D subset. Deliberate differences:
explicit ticks instead of d3-timer/d3-dispatch; bounded validated JSON input;
fixed constant charge rather than a JS callback; deterministic caller seed;
safe bounded octree subdivision; cooperative cancellation within ticks; an optional alpha
stop threshold. Coincident leaf insertions use append-then-reverse instead of
repeated front insertion, preserving the final upstream leaf traversal order
without quadratic construction cost. Cancellation checks do not reorder normal
floating-point operations. No timers, DOM, Canvas, ThreeJS renderer, particle effects or
1D/2D solver are copied into production. No independent collision solver is
claimed in this release. Native rendering, camera/picking and the ordered tree
layout belong to their separate application layers.

## Runtime dependencies

The Godot adapter uses unchanged `godot-rust/gdext` crates, version **0.4.5**,
under **MPL-2.0**, with minimal API generation and Godot API 4.3. Source is available
at https://github.com/godot-rust/gdext/tree/v0.4.5 . Their licenses and Cargo.lock
are shipped with the source package; they have not been relicensed as MIT. The
core uses serde / serde_json (MIT OR Apache-2.0). No upstream Rust dependency is
modified here. Generated Linux binaries are built from the accompanying sources.

## Test-only dependencies

`package-lock.json` locks d3-dispatch, d3-timer, d3-quadtree and d3-binarytree needed
to load the unchanged upstream reference. They are not used by the product.
Do not run lifecycle scripts when installing the oracle (`npm ci --ignore-scripts`).

