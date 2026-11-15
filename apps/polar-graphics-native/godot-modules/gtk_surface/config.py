def can_build(env, platform):
    return platform in ("linuxbsd", "macos") and env["opengl3"] and env["library_type"] == "shared_library"

def configure(env):
    if env["platform"] == "linuxbsd":
        # The GTK host also exports bundled FreeType/HarfBuzz functions. Bind
        # engine-internal calls to Godot's tested bundled implementations;
        # otherwise host symbol interposition breaks its compressed UI font.
        env.AppendUnique(LINKFLAGS=["-Wl,-Bsymbolic-functions"])
