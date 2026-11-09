def can_build(env, platform):
    return platform in ("linuxbsd", "macos") and env["opengl3"] and env["library_type"] == "shared_library"

def configure(env):
    pass
