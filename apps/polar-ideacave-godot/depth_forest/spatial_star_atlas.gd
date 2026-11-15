extends Node3D
## Presentation-only atlas of the existing immutable forest; no synthetic nodes.
var star_ids: Array=[]
var atlas_points: Dictionary={}
var heat_levels: Dictionary={}
var stars: MultiMeshInstance3D
var material: ShaderMaterial
var legend: Label
var world_points: Dictionary={}
var sources: Dictionary={}
var _motion_state = null

func configure(points: Dictionary,source_by_node: Dictionary) -> void:
	world_points=points;sources=source_by_node
	star_ids=points.keys();star_ids.sort()
	var rectangle:=Rect2();var first:=true
	for id in star_ids:
		var p: Vector3=points[id]
		var atlas:=Vector2(p.x-p.z*.22,-p.y+p.z*.08)
		atlas_points[id]=atlas
		rectangle=Rect2(atlas,Vector2.ZERO) if first else rectangle.expand(atlas);first=false
	for id in star_ids:
		atlas_points[id]=Vector2(.05,.13)+(atlas_points[id]-rectangle.position)/Vector2(maxf(1,rectangle.size.x),maxf(1,rectangle.size.y))*Vector2(.90,.72)
	stars=MultiMeshInstance3D.new();stars.cast_shadow=GeometryInstance3D.SHADOW_CASTING_SETTING_OFF
	var multimesh:=MultiMesh.new();multimesh.transform_format=MultiMesh.TRANSFORM_3D;multimesh.use_custom_data=true
	multimesh.mesh=QuadMesh.new();multimesh.instance_count=star_ids.size()
	multimesh.custom_aabb=AABB(Vector3(-20000,-20000,-20000),Vector3(40000,40000,40000))
	for i in star_ids.size():multimesh.set_instance_transform(i,Transform3D.IDENTITY)
	stars.multimesh=multimesh
	material=ShaderMaterial.new();material.shader=preload("res://addons/polar_graphics/depth/spatial_star_atlas.gdshader");material.render_priority=-100
	stars.material_override=material;add_child(stars)
	legend=Label.new();legend.mouse_filter=Control.MOUSE_FILTER_IGNORE;legend.add_theme_font_size_override("font_size",13)
	legend.modulate=Color("95acb6");add_child(legend)

func refresh(camera: Camera3D,viewport_size: Vector2,active_source: String,reading_region: Rect2,reduced_motion: bool) -> void:
	if not stars:return
	var hot_count:=0
	for i in star_ids.size():
		var id: String=star_ids[i];var heat:=.14
		if sources.get(id,"")==active_source:
			heat=.55
			if camera.is_position_in_frustum(world_points[id]) and reading_region.has_point(camera.unproject_position(world_points[id])):heat=1.0;hot_count+=1
		if heat_levels.get(id,-1.0)!=heat:
			heat_levels[id]=heat
			var point: Vector2=atlas_points[id]
			stars.multimesh.set_instance_custom_data(i,Color(point.x,point.y,float(i)/maxi(1,star_ids.size()),heat))
	if _motion_state != reduced_motion:
		_motion_state = reduced_motion
		material.set_shader_parameter("animate_stars",not reduced_motion)
	legend.text="Star atlas · %d nodes   Blue: all · Amber: tree · Gold: %d in view"%[star_ids.size(),hot_count]
	legend.position=Vector2(maxf(700,viewport_size.x-620),viewport_size.y-32)
