#include "core/extension/godot_instance.h"
#include "core/extension/libgodot.h"
#include "core/input/input.h"
#include "core/os/os.h"
#include "main/main.h"
#include "core/input/input_event.h"
#include "drivers/gles3/rasterizer_gles3.h"
#include "scene/main/scene_tree.h"
#include "scene/main/node.h"
#include "scene/main/window.h"
#include "scene/main/viewport.h"
#include "scene/resources/packed_scene.h"
#ifdef MACOS_ENABLED
#include "scene/resources/font.h"
#include "scene/theme/theme_db.h"
#endif
#include "core/io/resource_loader.h"
#include "core/io/json.h"
#include "core/object/callable_mp.h"
#include "servers/rendering/rendering_server.h"
#include "servers/display/display_server_headless.h"

// The host owns context/FBO creation. No GDK dependency enters Godot.
class GtkSurfaceDisplay : public DisplayServerHeadless {
    Size2i size;
    Callable rect_changed;
public:
    explicit GtkSurfaceDisplay(Size2i p_size) : size(p_size) {}
    String get_name() const override { return "gtk_surface"; }
    float screen_get_scale(int = DisplayServerEnums::SCREEN_OF_MAIN_WINDOW) const override { return surface_scale_value; }
    static float surface_scale_value;
    int get_screen_count() const override { return 1; }
    Size2i screen_get_size(int = DisplayServerEnums::SCREEN_OF_MAIN_WINDOW) const override { return size; }
    Rect2i screen_get_usable_rect(int = DisplayServerEnums::SCREEN_OF_MAIN_WINDOW) const override { return Rect2i(Point2i(), size); }
    Vector<DisplayServerEnums::WindowID> get_window_list() const override { return {DisplayServerEnums::MAIN_WINDOW_ID}; }
    int window_get_current_screen(DisplayServerEnums::WindowID = DisplayServerEnums::MAIN_WINDOW_ID) const override { return 0; }
    Size2i window_get_size(DisplayServerEnums::WindowID = DisplayServerEnums::MAIN_WINDOW_ID) const override { return size; }
    Size2i window_get_size_with_decorations(DisplayServerEnums::WindowID = DisplayServerEnums::MAIN_WINDOW_ID) const override { return size; }
    void window_set_size(Size2i p_size, DisplayServerEnums::WindowID = DisplayServerEnums::MAIN_WINDOW_ID) override {
        if (size == p_size) return;
        size = p_size;
        if (rect_changed.is_valid()) rect_changed.call(Rect2i(Point2i(), size));
    }
    void window_set_rect_changed_callback(const Callable &p_callback, DisplayServerEnums::WindowID = DisplayServerEnums::MAIN_WINDOW_ID) override { rect_changed = p_callback; }
    DisplayServerEnums::WindowMode window_get_mode(DisplayServerEnums::WindowID = DisplayServerEnums::MAIN_WINDOW_ID) const override { return DisplayServerEnums::WINDOW_MODE_WINDOWED; }
    bool window_can_draw(DisplayServerEnums::WindowID = DisplayServerEnums::MAIN_WINDOW_ID) const override { return true; }
    bool can_any_window_draw() const override { return true; }
    void gl_window_make_current(DisplayServerEnums::WindowID) override {} // Already current on GTK thread.
    void swap_buffers() override {} // GTK composites after the render callback.
    DisplayServerEnums::VSyncMode window_get_vsync_mode(DisplayServerEnums::WindowID) const override { return DisplayServerEnums::VSYNC_DISABLED; }
    static Vector<String> drivers() { return {"opengl3"}; }
    static DisplayServer *create(const String &, DisplayServerEnums::WindowMode, DisplayServerEnums::VSyncMode, uint32_t, const Vector2i *, const Vector2i &resolution, int, DisplayServerEnums::Context, int64_t, Error &error) {
        error = OK;
        RasterizerGLES3::make_current(true);
        return memnew(GtkSurfaceDisplay(resolution));
    }
};

float GtkSurfaceDisplay::surface_scale_value = 1;
static GodotInstance *instance;
static float surface_scale = 1;
using HostEvent = void (*)(const char *, void *);
struct EmbeddedView { SubViewport *viewport = nullptr; bool visible = true; bool dirty = true; HostEvent callback = nullptr; void *context = nullptr; };
static HashMap<uint64_t, EmbeddedView> views;
static uint64_t next_view = 1;
static SubViewport *input_view = nullptr;
static GLuint present_fbo = 0;
static void deliver_input(const Ref<InputEvent> &event) {
    if (input_view) input_view->push_input(event, true);
    else Input::get_singleton()->parse_input_event(event);
}
static uint64_t readback_calls;
static PFNGLREADPIXELSPROC real_read_pixels;
static PFNGLGETTEXIMAGEPROC real_get_tex_image;
static void GLAD_API_PTR count_read_pixels(GLint x, GLint y, GLsizei w, GLsizei h, GLenum format, GLenum type, void *data) {
    ++readback_calls;
    real_read_pixels(x,y,w,h,format,type,data);
}
static void GLAD_API_PTR count_get_tex_image(GLenum target, GLint level, GLenum format, GLenum type, void *data) {
    ++readback_calls;
    real_get_tex_image(target,level,format,type,data);
}
static void init_level(void *, GDExtensionInitializationLevel) {}
static GDExtensionBool init_extension(GDExtensionInterfaceGetProcAddress, GDExtensionClassLibraryPtr, GDExtensionInitialization *init) {
    init->minimum_initialization_level = GDEXTENSION_INITIALIZATION_CORE;
    init->initialize = init_level;
    init->deinitialize = init_level;
    init->userdata = nullptr;
    return true;
}

extern "C" LIBGODOT_API int polar_godot_start(int argc, char **argv, int width, int height, unsigned int fbo) {
    if (instance || !fbo || width < 1 || height < 1) return 0;
    DisplayServer::register_create_function("gtk_surface", GtkSurfaceDisplay::create, GtkSurfaceDisplay::drivers);
    GLES3::TextureStorage::system_fbo = fbo;
    instance = static_cast<GodotInstance *>(libgodot_create_godot_instance(argc, argv, init_extension));
    if (!instance || !instance->start()) return 0;
#ifdef MACOS_ENABLED
    // CoreText implicit fallback can choose an unavailable downloadable font.
    // Use the installed CJK family through Godot's existing SystemFont loader.
    Ref<SystemFont> font;
    font.instantiate();
    font->set_font_names({"Hiragino Sans GB", "Heiti SC", "Arial Unicode MS", "sans-serif"});
    ThemeDB::get_singleton()->set_fallback_font(font);
    ThemeDB::get_singleton()->get_default_theme()->set_default_font(font);
#endif
    DisplayServer::get_singleton()->window_set_size(Size2i(width, height));
    real_read_pixels = glad_glReadPixels;
    real_get_tex_image = glad_glGetTexImage;
    glad_glReadPixels = count_read_pixels;
    glad_glGetTexImage = count_get_tex_image;
    return 1;
}
extern "C" LIBGODOT_API void polar_godot_surface_scale(float scale) {
    surface_scale = scale;
    GtkSurfaceDisplay::surface_scale_value = scale;
    if (instance && SceneTree::get_singleton() && SceneTree::get_singleton()->get_root() && SceneTree::get_singleton()->get_root()->get_content_scale_factor() != scale)
        SceneTree::get_singleton()->get_root()->set_content_scale_factor(scale);
}
extern "C" LIBGODOT_API int polar_godot_needs_frame() {
    if (!instance) return 1;
    Node *scene = SceneTree::get_singleton()->get_current_scene();
    return !scene || !scene->has_method("needs_continuous_frames") || bool(scene->call("needs_continuous_frames"));
}
extern "C" LIBGODOT_API int polar_godot_frame(int width, int height, unsigned int fbo) {
    if (!instance || !fbo) return 1;
    GLES3::TextureStorage::system_fbo = fbo;
    DisplayServer::get_singleton()->window_set_size(Size2i(width, height));
    polar_godot_surface_scale(surface_scale);
    Main::force_redraw(); // GTK presents a fresh framebuffer for every render callback.
    bool done = instance->iteration();
    return done ? (OS::get_singleton()->get_exit_code() ? -1 : 1) : 0;
}
extern "C" LIBGODOT_API void polar_godot_stop() {
    if (!instance) return;
    views.clear();
    input_view = nullptr;
    if (present_fbo) { glDeleteFramebuffers(1, &present_fbo); present_fbo = 0; }
    libgodot_destroy_godot_instance(instance);
    instance = nullptr;
}
extern "C" LIBGODOT_API uint64_t polar_godot_readbacks() { return readback_calls; }

extern "C" LIBGODOT_API void polar_godot_pointer_event(int kind, double x, double y, double dx, double dy, int button, int pressed, unsigned int mask, unsigned int modifiers, int double_click) {
    if (!instance) return;
    if (kind == 0) {
        Ref<InputEventMouseMotion> event;
        event.instantiate();
        event->set_position(Vector2(x,y));
        event->set_global_position(Vector2(x,y));
        event->set_relative(Vector2(dx,dy));
        event->set_button_mask(static_cast<MouseButtonMask>(mask));
        event->set_shift_pressed(modifiers & 1);
        event->set_ctrl_pressed(modifiers & 4);
        event->set_alt_pressed(modifiers & 8);
        deliver_input(event);
    } else {
        Ref<InputEventMouseButton> event;
        event.instantiate();
        event->set_position(Vector2(x,y));
        event->set_global_position(Vector2(x,y));
        event->set_button_index(static_cast<MouseButton>(button));
        if (kind == 3) event->set_factor(Math::abs(dy));
        event->set_button_mask(static_cast<MouseButtonMask>(mask));
        event->set_double_click(double_click);
        event->set_shift_pressed(modifiers & 1);
        event->set_ctrl_pressed(modifiers & 4);
        event->set_alt_pressed(modifiers & 8);
        event->set_pressed(pressed);
        deliver_input(event);
    }
}
extern "C" LIBGODOT_API void polar_godot_pointer(int kind, double x, double y, double dx, double dy, int button, int pressed) {
    polar_godot_pointer_event(kind,x,y,dx,dy,button,pressed,pressed ? 1u : 0u,0,0);
}
extern "C" LIBGODOT_API void polar_godot_magnify(double x, double y, double factor, unsigned int modifiers) {
    if (!instance || !Math::is_finite(factor) || factor <= 0) return;
    Ref<InputEventMagnifyGesture> event;
    event.instantiate(); event->set_position(Vector2(x,y)); event->set_factor(factor);
    event->set_shift_pressed(modifiers & 1); event->set_ctrl_pressed(modifiers & 4); event->set_alt_pressed(modifiers & 8);
    deliver_input(event);
}
extern "C" LIBGODOT_API void polar_godot_pan(double x, double y, double dx, double dy, unsigned int modifiers) {
    if (!instance || !Math::is_finite(dx) || !Math::is_finite(dy)) return;
    Ref<InputEventPanGesture> event;
    event.instantiate(); event->set_position(Vector2(x,y)); event->set_delta(Vector2(dx,dy));
    event->set_shift_pressed(modifiers & 1); event->set_ctrl_pressed(modifiers & 4); event->set_alt_pressed(modifiers & 8);
    deliver_input(event);
}
extern "C" LIBGODOT_API void polar_godot_key(unsigned int unicode, unsigned int keyval, int pressed, unsigned int modifiers) {
    if (!instance) return;
    Key key = static_cast<Key>(keyval >= 'a' && keyval <= 'z' ? keyval - 32 : keyval);
    switch (keyval) {
        case 0xff0d: key=Key::ENTER; break;
        case 0xff1b: key=Key::ESCAPE; break;
        case 0xff08: key=Key::BACKSPACE; break;
        case 0xff09: key=Key::TAB; break;
        case 0xff51: key=Key::LEFT; break;
        case 0xff52: key=Key::UP; break;
        case 0xff53: key=Key::RIGHT; break;
        case 0xff54: key=Key::DOWN; break;
        case 0xffff: key=Key::KEY_DELETE; break;
    }
    Ref<InputEventKey> event;
    event.instantiate();
    event->set_keycode(key);
    event->set_unicode(unicode);
    event->set_pressed(pressed);
    event->set_shift_pressed(modifiers & 1);
    event->set_ctrl_pressed(modifiers & 4);
    event->set_alt_pressed(modifiers & 8);
    deliver_input(event);
}
extern "C" LIBGODOT_API int polar_godot_check_state() {
    if (!instance) return 0;
    Node *scene = SceneTree::get_singleton()->get_current_scene();
    if (!scene) return 0;
    return !String(scene->get("focused_id")).is_empty() && bool(scene->get("opened"));
}

#include "register_types.h"
void initialize_gtk_surface_module(ModuleInitializationLevel level) {}
void uninitialize_gtk_surface_module(ModuleInitializationLevel level) {}

extern "C" LIBGODOT_API void polar_godot_camera(double *yaw, double *distance) {
    if (!instance) return;
    Node *scene = SceneTree::get_singleton()->get_current_scene();
    if (!scene) return;
    *yaw = double(scene->get("orbit_yaw"));
    *distance = double(scene->get("wanted_distance"));
}

// Pushed scene state; callback strings are borrowed only for the duration of the call.
static void host_state_changed(Dictionary state, uint64_t id) {
    EmbeddedView *view = views.getptr(id);
    if (!view || !view->callback) return;
    Dictionary event; event["kind"] = "state"; event["state"] = state;
    CharString json = JSON::stringify(event).utf8();
    view->callback(json.get_data(), view->context);
}
static void host_focus_search(uint64_t id) {
    EmbeddedView *view = views.getptr(id);
    if (view && view->callback) view->callback("{\"kind\":\"focus_search\"}", view->context);
}
static void host_open_source(String path, uint64_t id) {
    EmbeddedView *view = views.getptr(id);
    if (!view || !view->callback) return;
    Dictionary event; event["kind"] = "open_source"; event["path"] = path;
    CharString json = JSON::stringify(event).utf8();
    view->callback(json.get_data(), view->context);
}
static void host_child_entered(Node *content, uint64_t id) {
    if (content->has_signal("host_open_source"))
        content->connect("host_open_source", callable_mp_static(host_open_source).bind(id));
    if (content->has_signal("host_state_changed"))
        content->connect("host_state_changed", callable_mp_static(host_state_changed).bind(id));
    if (content->has_signal("host_focus_search"))
        content->connect("host_focus_search", callable_mp_static(host_focus_search).bind(id));
}
extern "C" LIBGODOT_API void polar_godot_view_subscribe(uint64_t id, HostEvent callback, void *context) {
    EmbeddedView *view = views.getptr(id);
    if (!view) return;
    view->callback = callback; view->context = context;
    Node *content = view->viewport->get_child_count() ? view->viewport->get_child(0) : nullptr;
    if (callback && content && content->has_method("host_state")) host_state_changed(content->call("host_state"), id);
}
extern "C" LIBGODOT_API void polar_godot_view_command(uint64_t id, const char *action, const char *query) {
    EmbeddedView *view = views.getptr(id);
    if (!view || !action || !query) return;
    Node *content = view->viewport->get_child_count() ? view->viewport->get_child(0) : nullptr;
    if (!content || !content->has_method("host_command")) return;
    view->dirty = true;
    content->call("host_command", String::utf8(action), String::utf8(query));
}

// Stable view identity belongs to the caller; all calls run on the GTK GL thread.
extern "C" LIBGODOT_API uint64_t polar_godot_view_create(const char *identity, const char *scene) {
    if (!instance) return 0;
    Ref<PackedScene> packed = ResourceLoader::load(String::utf8(scene));
    if (packed.is_null()) return 0;
    Node *content = packed->instantiate();
    if (!content) return 0;
    SubViewport *viewport = memnew(SubViewport);
    viewport->set_size(Size2i(1, 1));
    viewport->set_embedding_subwindows(true);
    viewport->set_use_own_world_3d(true);
    viewport->set_update_mode(SubViewport::UPDATE_DISABLED);
    content->set_meta("view_id", String::utf8(identity));
    content->set_meta("native_controls", true);
    uint64_t id = next_view++;
    views.insert(id, EmbeddedView{viewport, true, true});
    viewport->connect("child_entered_tree", callable_mp_static(host_child_entered).bind(id));
    SceneTree::get_singleton()->get_root()->add_child(viewport);
    viewport->add_child(content);
    return id;
}
extern "C" LIBGODOT_API void polar_godot_view_configure(uint64_t id, int width, int height, float scale, int visible) {
    EmbeddedView *view = views.getptr(id);
    if (!view || width < 1 || height < 1 || scale <= 0) return;
    Size2i size(width, height), logical(MAX(1, int(width / scale)), MAX(1, int(height / scale)));
    // Unmap allocations may be zero or logical-sized. Retain the GPU target
    // while hidden; the next visible render supplies authoritative pixels/DPI.
    if (visible && (view->viewport->get_size() != size || view->viewport->get_size_2d_override() != logical)) {
        view->viewport->set_size(size);
        view->viewport->set_size_2d_override(logical);
        view->viewport->set_size_2d_override_stretch(true);
        view->dirty = true;
    }
    if (bool(visible) != view->visible) {
        view->dirty = true;
        Node *content = view->viewport->get_child_count() ? view->viewport->get_child(0) : nullptr;
        if (content && content->has_method("host_visibility_changed")) content->call("host_visibility_changed", bool(visible));
    }
    view->visible = visible;
    view->viewport->set_process_mode(visible ? Node::PROCESS_MODE_INHERIT : Node::PROCESS_MODE_DISABLED);
    if (!visible) view->viewport->set_update_mode(SubViewport::UPDATE_DISABLED);
}
extern "C" LIBGODOT_API int polar_godot_view_needs_frame(uint64_t id) {
    EmbeddedView *view = views.getptr(id);
    if (!view || !view->visible) return 0;
    Node *content = view->viewport->get_child_count() ? view->viewport->get_child(0) : nullptr;
    return view->dirty || (content && content->has_method("needs_continuous_frames") && bool(content->call("needs_continuous_frames")));
}
extern "C" LIBGODOT_API void polar_godot_view_input(uint64_t id) {
    EmbeddedView *view = views.getptr(id);
    input_view = view ? view->viewport : nullptr;
    if (view) view->dirty = true;
}
extern "C" LIBGODOT_API int polar_godot_step(unsigned int fbo) {
    if (!instance || !fbo) return 1;
    GLES3::TextureStorage::system_fbo = fbo;
    for (KeyValue<uint64_t, EmbeddedView> &entry : views) {
        if (polar_godot_view_needs_frame(entry.key)) {
            entry.value.viewport->set_update_mode(SubViewport::UPDATE_ONCE);
            entry.value.dirty = false;
        }
    }
    Main::force_redraw();
    return instance->iteration() ? 1 : 0;
}
extern "C" LIBGODOT_API int polar_godot_view_present_context(uint64_t id, int width, int height, unsigned int destination, unsigned int read_fbo) {
    EmbeddedView *view = views.getptr(id);
    if (!view || !read_fbo) return 0;
    RID texture = RenderingServer::get_singleton()->viewport_get_texture(view->viewport->get_viewport_rid());
    GLuint handle = GLuint(RenderingServer::get_singleton()->texture_get_native_handle(texture));
    if (!handle) { view->dirty = true; return 0; }
    GLint read = 0, draw = 0;
    glGetIntegerv(GL_READ_FRAMEBUFFER_BINDING, &read);
    glGetIntegerv(GL_DRAW_FRAMEBUFFER_BINDING, &draw);
    glBindFramebuffer(GL_READ_FRAMEBUFFER, read_fbo);
    glFramebufferTexture2D(GL_READ_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_TEXTURE_2D, handle, 0);
    if (glCheckFramebufferStatus(GL_READ_FRAMEBUFFER) != GL_FRAMEBUFFER_COMPLETE) {
        glBindFramebuffer(GL_READ_FRAMEBUFFER, read);
        glBindFramebuffer(GL_DRAW_FRAMEBUFFER, draw);
        view->dirty = true;
        return 0;
    }
    glBindFramebuffer(GL_DRAW_FRAMEBUFFER, destination);
    GLboolean scissor = glIsEnabled(GL_SCISSOR_TEST);
    glDisable(GL_SCISSOR_TEST);
    // Godot render-target textures and GTK framebuffer coordinates have opposite Y origins.
    glBlitFramebuffer(0, height, width, 0, 0, 0, width, height, GL_COLOR_BUFFER_BIT, GL_NEAREST);
    if (scissor) glEnable(GL_SCISSOR_TEST);
    glBindFramebuffer(GL_READ_FRAMEBUFFER, read);
    glBindFramebuffer(GL_DRAW_FRAMEBUFFER, draw);
    return 1;
}
// GTK uses one retained context. AppKit supplies a per-context read FBO, because
// framebuffer objects (unlike textures) are not shared across OpenGL contexts.
extern "C" LIBGODOT_API int polar_godot_view_present(uint64_t id, int width, int height, unsigned int destination) {
    if (!destination) return 0;
    if (!present_fbo) glGenFramebuffers(1, &present_fbo);
    return polar_godot_view_present_context(id, width, height, destination, present_fbo);
}
extern "C" LIBGODOT_API void polar_godot_view_destroy(uint64_t id) {
    EmbeddedView *view = views.getptr(id);
    if (!view) return;
    if (input_view == view->viewport) input_view = nullptr;
    view->callback = nullptr; view->context = nullptr;
    memdelete(view->viewport);
    views.erase(id);
}
