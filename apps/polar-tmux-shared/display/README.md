# Processless display submission boundary (K07, C03)

This is an internal C ABI for the paired managed-Ghostty adapter, not a renderer,
TUI, font implementation, terminal process, independent queue or message model.
It admits only bounded safe UTF-8 text and calls a synchronous native sink.
Generation, content epoch and contiguous revision fences protect stale updates.
A failed batch invokes no sink; a sink failure must mean no output was committed.
The first batch after bind must replace the visible window; append cannot expose
another conversation's old text. Native code must conceal the old surface while
rebinding and install semantic hit maps only after the matching snapshot.

All methods run on the creator thread. No thread can use a handle concurrently
with destroy. Retire invalidates future batches, destroy consumes the handle;
FFI callers must zero it. Sinks must never retain the borrowed buffer or partially
commit then return an error. No application/clipboard/device-control sequences
are accepted. Logical LF/TAB are translated by the eventual trusted Ghostty sink.

**Integration boundary:** the managed Ghostty overlay and paired native adapters
call this guard; rendering is implemented in `../ghostty/StationDisplay.zig`.
The C tests use a recording sink and do not establish Ghostty rendering behavior
or native interaction acceptance. No function here launches a process or PTY.

Diagnostic test from repository root:
`python automation/tests/polar_station_display_boundary.py --output /mnt/data/display-tests`


Typed core-content operations 3/4 carry a bounded JSON array of semantic text
runs, using the same epoch/revision fences as operations 1/2. The C boundary
checks the envelope's UTF-8 and size; the shared Ghostty sink validates every
decoded string/style/link before generating SGR/OSC8 and synchronized output.
Malformed runs produce no terminal mutation. Render guards in the existing LGP
view state defer revisions during selection; native pointer/key events resume
rendering without a new polling loop. Viewport row offset is retained on redraw.
