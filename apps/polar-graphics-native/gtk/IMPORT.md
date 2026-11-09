# GTK surface provenance

Adapted from [iTree-View](https://github.com/lionantbear/iTree-View) commit `58bc62ae5e0c728a6853905e84ed7e1a8c285ef3`, `linux-libgodot/src/main.c` and `polar_godot.h`. The matching Godot custom module is preserved in `../godot-modules`; source hashes are in `../IMPORT.json`. Godot upstream is MIT licensed.

The adapter removes application/window ownership and retains the engine/context across widget lifetimes. The embedding application owns the widget and explicit final shutdown. `tests/lifecycle.c` exercises close/reopen using the same context. No CPU pixel readback is introduced.
