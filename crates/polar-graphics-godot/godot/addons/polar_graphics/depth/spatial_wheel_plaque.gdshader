shader_type canvas_item;
uniform vec4 accent : source_color = vec4(0.2, 0.85, 0.77, 1.0);
uniform vec2 card_size = vec2(160.0, 78.0);
uniform float emphasis = 0.0;
uniform float hover_amount = 0.0;
void fragment() {
    vec2 p = UV * card_size;
    float cut = 12.0;
    float edge = min(min(p.y, card_size.y-p.y), min(p.x - abs(p.y-card_size.y*0.5)*0.19, card_size.x-p.x - abs(p.y-card_size.y*0.5)*0.22));
    if (edge < 0.0) discard;
    float outer = 1.0 - smoothstep(0.6, 1.6, edge);
    float bevel = (1.0-smoothstep(2.0, 4.8, edge))*0.68;
    float inner = (1.0-smoothstep(5.2, 6.4, edge)) * smoothstep(4.0, 5.0, edge);
    float top = 1.0-UV.y;
    vec3 base = mix(vec3(0.040,0.073,0.095),vec3(0.095,0.143,0.164),top);
    base += accent.rgb * (0.025 + emphasis*0.075 + hover_amount*0.04);
    base += accent.rgb * exp(-edge/13.0)*emphasis*0.11;
    vec3 rim = mix(vec3(0.43,0.53,0.57),accent.rgb,0.45+emphasis*0.5);
    vec3 color = mix(base,rim,bevel*(0.55+top*0.38));
    color = mix(color,mix(vec3(0.65,0.77,0.79),accent.rgb,emphasis),outer*(0.58+top*0.35));
    color += accent.rgb*inner*(0.15+emphasis*0.55);
    float sheen = pow(max(0.0,1.0-abs(UV.y-0.17)*2.5),8.0);
    color += vec3(0.065)*sheen;
    COLOR = vec4(color,1.0);
}
