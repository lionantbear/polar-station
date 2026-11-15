# GTK surface provenance

Adapted from [iTree-View](https://github.com/lionantbear/iTree-View) commit `58bc62ae5e0c728a6853905e84ed7e1a8c285ef3`, `linux-libgodot/src/main.c` and `polar_godot.h`. The matching Godot custom module is preserved in `../godot-modules`; source hashes are in `../IMPORT.json`. Godot upstream is MIT licensed.

The adapter removes application/window ownership and retains the engine/context across widget lifetimes. The embedding application owns the widget and explicit final shutdown. `tests/lifecycle.c` exercises close/reopen using the same context. No CPU pixel readback is introduced.

Native Mobile presentation is adapted from iTree-View `8f4bf461b7d7187b3b20df2b5422300bc445e48d`, `experiments/native-rd/`; source hashes are appended in `../IMPORT.json`. Linux renders through Vulkan and imports a same-device external image into GTK's GL surface, with GPU semaphore ownership transfers. macOS presents the shared Metal texture through MTKView on Godot's command queue. Existing product scenes, native input and file-picker ownership are retained. GPU waits are confined to resource-lifetime boundaries; idle readiness uses OS events and one-shot deadlines.
