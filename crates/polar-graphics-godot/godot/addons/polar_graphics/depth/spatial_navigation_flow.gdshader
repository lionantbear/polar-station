shader_type spatial;
render_mode unshaded, cull_disabled, blend_add, depth_draw_never;
uniform bool animate_flow = true;
uniform bool semantic_route = true;
uniform vec4 canvas_region = vec4(0.0,0.0,1.0,1.0);
void fragment() {
    if (SCREEN_UV.x < canvas_region.x || SCREEN_UV.y < canvas_region.y || SCREEN_UV.x > canvas_region.z || SCREEN_UV.y > canvas_region.w) discard;
    float phase = fract(UV.x / 5.0 - (animate_flow ? TIME * 0.65 : 0.0));
    float traveling = pow(phase, 7.0);
    float across = abs(UV.y);
    float core = 1.0-smoothstep(0.20,0.45,across);
    float halo = pow(max(0.0,1.0-across),2.4);
    float dash = semantic_route ? 1.0 : step(0.22,fract(UV.x*1.3));
    vec3 tone = mix(vec3(1.0,0.62,0.24),vec3(0.30,0.90,1.0),UV2.x);
    ALBEDO = mix(tone,vec3(0.9,1.0,1.0),traveling*0.6);
    ALPHA = dash*(core*(0.22+traveling*0.78)+halo*(0.08+traveling*0.35));
}
