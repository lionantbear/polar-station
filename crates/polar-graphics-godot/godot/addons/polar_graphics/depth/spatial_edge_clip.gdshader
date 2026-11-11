shader_type spatial;
stencil_mode read, compare_not_equal, 1;
render_mode unshaded, cull_disabled;
uniform vec4 canvas_region = vec4(0.0, 0.0, 1.0, 1.0);
varying vec4 line_color;
void vertex() { line_color = COLOR; }
void fragment() {
    // Graph routes never appear attached to the camera-mounted source wheel
    // or run through the header. Node identities and world paths are intact.
    if (SCREEN_UV.x < canvas_region.x || SCREEN_UV.y < canvas_region.y ||
        SCREEN_UV.x > canvas_region.z || SCREEN_UV.y > canvas_region.w) { discard; }
    ALBEDO = line_color.rgb;
    ALPHA = line_color.a;
}
