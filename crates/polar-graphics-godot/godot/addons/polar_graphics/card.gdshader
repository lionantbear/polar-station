shader_type spatial;
render_mode unshaded, cull_disabled;
uniform vec2 rect_size = vec2(1.8, 0.54);
uniform vec4 accent : source_color = vec4(0.4, 0.8, 0.7, 1.0);
uniform vec4 base_color : source_color = vec4(0.145, 0.176, 0.220, 1.0);
uniform bool selected = false;
uniform float context_strength = 1.0;
uniform bool silhouette = false;
uniform bool compact = false;
uniform vec4 selection_outline : source_color = vec4(0.302, 0.878, 0.8, 1.0);

void vertex() {
	// Cached glyphs and cards share the camera-facing basis. Source-layer
	// position remains three-dimensional while typography stays front-facing.
	MODELVIEW_MATRIX = VIEW_MATRIX * mat4(INV_VIEW_MATRIX[0], INV_VIEW_MATRIX[1], INV_VIEW_MATRIX[2], MODEL_MATRIX[3]);
}

void fragment() {
	vec2 p = (UV - vec2(0.5)) * rect_size;
	float radius = compact ? 0.08 : 0.09;
	vec2 q = abs(p) - (rect_size * 0.5 - vec2(radius));
	float d = length(max(q, vec2(0.0))) + min(max(q.x, q.y), 0.0) - radius;
	if (d > 0.0) discard;
	if (silhouette && d < -0.015) discard;
	float stroke = selected ? 0.022 : 0.010;
	vec3 base = mix(base_color.rgb, accent.rgb, compact ? 0.25 : 0.10);
	vec3 outline = selected ? selection_outline.rgb : mix(base, accent.rgb, 0.5);
	// Source stays visible inside the independent selection contour.
	ALBEDO = (d > -stroke ? outline : (p.x < -rect_size.x * 0.5 + 0.045 ? accent.rgb : base)) * context_strength;
}
