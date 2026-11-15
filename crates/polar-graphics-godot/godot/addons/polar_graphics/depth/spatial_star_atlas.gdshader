shader_type spatial;
render_mode unshaded, cull_disabled, blend_add, depth_draw_never, shadows_disabled, fog_disabled;
uniform bool animate_stars = true;
varying float heat;
varying float radius_px;
varying float phase;
void vertex() {
    heat=INSTANCE_CUSTOM.a;
    phase=INSTANCE_CUSTOM.z*47.0;
    radius_px=mix(32.0,160.0,heat);
    vec2 center=INSTANCE_CUSTOM.xy*2.0-1.0;
    // Atlas coordinates have a top-left origin. Use the native camera's Y
    // correction: Mobile/Forward+ and Compatibility do not share clip-space Y.
    center.y *= -sign(PROJECTION_MATRIX[1][1]);
    vec2 offset=(UV*2.0-1.0)*radius_px*2.0/VIEWPORT_SIZE;
    POSITION=vec4(center+offset,CLIP_SPACE_FAR+0.000001,1.0);
}
void fragment() {
    vec2 p=(UV*2.0-1.0)*radius_px;
    float r=length(p);
    float hot=smoothstep(0.55,1.0,heat);
    float warm=smoothstep(0.15,0.55,heat);
    vec3 tone=mix(vec3(0.23,0.46,0.80),vec3(0.80,0.31,0.08),warm);
    tone=mix(tone,vec3(1.0,0.77,0.30),hot);
    float core=1.0-smoothstep(0.7+heat,1.8+heat,r);
    float halo=exp(-r*r/(radius_px*radius_px*0.15));
    float inner=exp(-r*r/26.0);
    float rays=exp(-abs(p.x)*3.0)*exp(-abs(p.y)/7.0)+exp(-abs(p.y)*3.0)*exp(-abs(p.x)/7.0);
    float twinkle=animate_stars ? 0.94+0.06*sin(TIME*0.6+phase) : 1.0;
    ALBEDO=mix(tone,vec3(0.83,0.94,1.0),core*0.8);
    ALPHA=twinkle*(core*(0.3+heat*0.45)+inner*0.12+halo*(0.018+heat*0.10)+rays*hot*0.12);
}
