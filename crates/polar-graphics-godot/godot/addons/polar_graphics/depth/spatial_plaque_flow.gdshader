shader_type spatial;
uniform vec4 theme_metal_tint : source_color = vec4(1.0);
uniform float theme_roughness = 0.0;
render_mode cull_back;
instance uniform vec4 accent : source_color = vec4(0.3,0.85,0.78,1.0);
instance uniform float selected = 0.0;
instance uniform float hovered = 0.0;
instance uniform bool motion = false;
instance uniform float context_fade = 0.0;
instance uniform float navigation_role = 0.0;
instance uniform bool navigation_motion = false;
void fragment() {
    float traveling = motion ? pow(max(0.0,cos(UV.x*6.28318-TIME*1.25)),22.0) : 0.32;
    // Two polished lips separated by the real sloping bevel. This is still
    // its original narrow geometry, not an added outline or glowing face.
    float lip = 1.0-smoothstep(0.12,0.38,UV.y);
    float inner = smoothstep(0.73,0.94,UV.y);
    ALBEDO = mix(vec3(0.34,0.44,0.49),vec3(0.78,0.87,0.90),lip*0.72+inner*0.23);
    ALBEDO *= theme_metal_tint.rgb;
    ALBEDO = mix(ALBEDO,accent.rgb,0.10+selected*0.32);
    ALBEDO *= mix(1.0,0.28,context_fade);
    METALLIC = mix(0.78,0.15,context_fade);
    ROUGHNESS = mix(clamp(mix(0.29,0.16,lip)+theme_roughness,0.0,1.0),0.8,context_fade);
    // A restrained base makes selection legible at reading distance. The
    // brighter traveling highlight stays on the inner lip; no face emission.
    EMISSION = (1.0-context_fade)*accent.rgb*(selected*(0.75+inner*traveling*1.1)+(0.25+lip*0.75)*hovered*0.22);
    vec3 navigation_color = navigation_role < 0.0 ? vec3(1.0,0.65,0.24) : vec3(0.35,0.91,1.0);
    float sweep = navigation_motion ? pow(max(0.0,cos(UV.x*12.56636-TIME*2.8)),10.0) : 0.35;
    // History stays subordinate to selection. Arrival uses the selected
    // node's existing emphasis rather than competing with another hue.
    EMISSION += max(-navigation_role,0.0)*navigation_color*(0.10+inner*sweep*0.28)*(1.0-selected);
}
