extends Node3D
## Local geometry only. No model identity, navigation or source state is owned here.
const EdgeShader = preload("res://addons/polar_graphics/depth/spatial_plaque_flow.gdshader")
const TITLE_PIXEL_SIZE:=.0077
const FRONT_Z:=.18
const IconMesh=preload("res://addons/polar_graphics/depth/spatial_icon_mesh.gd")
var dimensions := Vector2(2.8,0.9)
var accent := Color("5cd9ce")
var title := ""
var glyph := "◇"
var glyph_kind:=0
var chosen := false
var motion := false
var face_material: StandardMaterial3D
var bevel_material: ShaderMaterial
var title_label: Label3D
var glyph_label
var skin: Node3D
var effect: Tween
var base_position := Vector3.ZERO
var context_dim:=false
var was_pressed:=false
var context_strength:=0.0
var visual_theme: Resource

static func dimensions_for_title(value: String) -> Vector2:
	# Shape once with the same metrics as the Label3D, before Graphviz layout.
	# Text remains complete; zoom never changes font metrics or reruns layout.
	var width:=6.6
	var text_width: float=(width-.96)/TITLE_PIXEL_SIZE
	var flags:=TextServer.BREAK_MANDATORY|TextServer.BREAK_WORD_BOUND|TextServer.BREAK_ADAPTIVE
	var measured: Vector2=ThemeDB.fallback_font.get_multiline_string_size(value,HORIZONTAL_ALIGNMENT_CENTER,text_width,64,-1,flags)
	return Vector2(width,maxf(1.65,measured.y*TITLE_PIXEL_SIZE+.36))

static func shape(dimensions: Vector2, inset: float=0.0) -> PackedVector2Array:
	var w := dimensions.x*0.5-inset
	var h := dimensions.y*0.5-inset
	var cut := minf(h*0.40,0.16)
	return PackedVector2Array([Vector2(-w,h-cut),Vector2(-w+cut,h),Vector2(w-cut,h),Vector2(w,h-cut),Vector2(w,-h+cut),Vector2(w-cut,-h),Vector2(-w+cut,-h),Vector2(-w,-h+cut)])

static func mesh_surface(outer: PackedVector2Array, inner: PackedVector2Array, z_outer: float, z_inner: float, fill: bool=false, reverse: bool=false) -> ArrayMesh:
	var key := "res://.polar-graphics/plaque/" + var_to_str([outer,inner,z_outer,z_inner,fill,reverse]).sha256_text()
	var existing: ArrayMesh = ResourceLoader.get_cached_ref(key) as ArrayMesh
	if existing != null:return existing
	var vertices := PackedVector3Array()
	var normals := PackedVector3Array()
	var uvs := PackedVector2Array()
	for i in outer.size():
		var next := (i+1)%outer.size()
		var a := Vector3(outer[i].x,outer[i].y,z_outer)
		var b := Vector3(outer[next].x,outer[next].y,z_outer)
		var c := Vector3(inner[next].x,inner[next].y,z_inner) if not fill else Vector3(0,0,z_inner)
		var d := Vector3(inner[i].x,inner[i].y,z_inner) if not fill else c
		var n := (c-a).cross(b-a).normalized()
		if reverse:n=-n
		var indices: Array=[0,1,2] if fill else [0,1,2,0,2,3]
		if reverse:indices=[0,2,1] if fill else [0,2,1,0,3,2]
		for index in indices:
			vertices.append([a,b,c,d][index]);normals.append(n)
			if fill:
				var point: Vector3=[a,b,c,d][index]
				var half_w:=maxf(.001,absf(outer[0].x));var half_h:=maxf(.001,absf(outer[1].y))
				uvs.append(Vector2(point.x/(2*half_w)+.5,.5-point.y/(2*half_h)))
			else:uvs.append(Vector2(float(i)/outer.size() if index in [0,3] else float(i+1)/outer.size(),0 if index in [0,1] else 1))
	var arrays: Array=[];arrays.resize(Mesh.ARRAY_MAX)
	arrays[Mesh.ARRAY_VERTEX]=vertices;arrays[Mesh.ARRAY_NORMAL]=normals;arrays[Mesh.ARRAY_TEX_UV]=uvs
	var mesh:=ArrayMesh.new();mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES,arrays)
	mesh.take_over_path(key)
	return mesh

static func glass_texture(dimensions: Vector2, unit: float) -> Texture2D:
	# A shared, construction-only laminate texture. Lighting/clearcoat remain
	# real StandardMaterial3D responses; no screen-space glow or frame work.
	var key := "res://.polar-graphics/glass/" + var_to_str([dimensions,unit]).sha256_text()
	var existing: Texture2D = ResourceLoader.get_cached_ref(key) as Texture2D
	if existing != null:return existing
	var image: Image
	if ClassDB.class_exists("PolarPlaqueRaster"):
		var contour:=shape(dimensions,.060*unit)
		var half_size:=Vector2(absf(contour[0].x),absf(contour[1].y))
		var raster=ClassDB.instantiate("PolarPlaqueRaster")
		var bytes:PackedByteArray=raster.glass_rgba8(contour,half_size,unit)
		image=Image.create_from_data(512,128,false,Image.FORMAT_RGBA8,bytes)
	else:
		# Compatibility for older standalone experiment extensions.
		image=_glass_reference_image(dimensions,unit)
	image.generate_mipmaps()
	var texture:=ImageTexture.create_from_image(image)
	texture.take_over_path(key)
	return texture

static func _glass_reference_image(dimensions:Vector2,unit:float)->Image:
	var contour:=shape(dimensions,.060*unit)
	var half_size:=Vector2(absf(contour[0].x),absf(contour[1].y))
	var image:=Image.create(512,128,false,Image.FORMAT_RGBA8)
	for y in image.get_height():
		for x in image.get_width():
			var uv:=Vector2(float(x)/(image.get_width()-1),float(y)/(image.get_height()-1))
			var point:=Vector2((uv.x-.5)*2*half_size.x,(.5-uv.y)*2*half_size.y)
			var distance:=INF
			for i in contour.size():
				var a:=contour[i];var edge:=contour[(i+1)%contour.size()]-a
				distance=minf(distance,point.distance_to(a+edge*clampf((point-a).dot(edge)/edge.length_squared(),0,1)))
			var tone:=Color("263945").lerp(Color("101c26"),smoothstep(0.0,.85,uv.y))
			# Dark recessed gasket and a fine inner glass seal, wholly in the
			# existing front UVs. The top seal catches more studio light.
			var gasket:=1.0-smoothstep(.007*unit,.027*unit,distance)
			var seal:=1.0-smoothstep(.005*unit,.014*unit,absf(distance-.043*unit))
			tone=tone.lerp(Color("080f17"),gasket*.65)
			tone=tone.lerp(Color("78939c"),seal*lerpf(.26,.08,uv.y))
			image.set_pixel(x,y,tone)
	return image

func _ready() -> void:
	skin=Node3D.new();add_child(skin)
	var outer:=shape(dimensions)
	var unit:=minf(1.0,dimensions.y/1.28)
	var inner:=shape(dimensions,0.060*unit)
	var side:=StandardMaterial3D.new()
	side.albedo_color=Color("496273");side.metallic=0.48;side.roughness=0.30
	side.cull_mode=BaseMaterial3D.CULL_BACK
	_add(mesh_surface(outer,outer,-.26*unit,.10*unit),side,"SideWalls")
	var rear:=StandardMaterial3D.new();rear.albedo_color=Color("101c24");rear.roughness=0.6;rear.cull_mode=BaseMaterial3D.CULL_BACK
	_add(mesh_surface(outer,outer,-.26*unit,-.26*unit,true,true),rear,"RearShell")
	bevel_material=ShaderMaterial.new();bevel_material.shader=EdgeShader
	bevel_material.set_shader_parameter("accent",accent)
	_add(mesh_surface(outer,inner,.10*unit,FRONT_Z*unit),bevel_material,"BevelRing")
	face_material=StandardMaterial3D.new()
	face_material.metallic=0.0;face_material.metallic_specular=.18
	face_material.roughness=0.23;face_material.clearcoat_enabled=true;face_material.clearcoat=0.40;face_material.clearcoat_roughness=0.16;face_material.cull_mode=BaseMaterial3D.CULL_BACK
	face_material.albedo_texture=glass_texture(dimensions,unit)
	_add(mesh_surface(inner,inner,FRONT_Z*unit,FRONT_Z*unit,true),face_material,"GlassFront")
	title_label=Label3D.new();title_label.font=ThemeDB.fallback_font;title_label.font_size=64;title_label.pixel_size=TITLE_PIXEL_SIZE*unit
	title_label.position=Vector3(0.25*unit,0,(FRONT_Z+.025)*unit);title_label.width=(dimensions.x-0.96*unit)/title_label.pixel_size
	title_label.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART;title_label.outline_size=0
	title_label.modulate=Color("e1eeee");title_label.text=title;title_label.render_priority=2
	title_label.no_depth_test=false;skin.add_child(title_label)
	glyph_label=IconMesh.new();glyph_label.kind=glyph_kind;glyph_label.tint=accent
	glyph_label.position=Vector3(-dimensions.x*.5+.32*unit,0,(FRONT_Z+.03)*unit)
	glyph_label.scale=Vector3.ONE*.47*unit;skin.add_child(glyph_label)
	set_selected(chosen,motion)
	pose(false,false,Vector2.ZERO,true)

func _add(mesh: Mesh, material: Material, label: String) -> void:
	var part:=MeshInstance3D.new();part.name=label;part.mesh=mesh;part.material_override=material;skin.add_child(part)

func resize_geometry(value: Vector2) -> void:
	# Mode-local context sizing reuses materials and the measured title; no new
	# identity, shader, texture generation or full forest layout is needed.
	if dimensions.is_equal_approx(value):return
	dimensions=value
	var outer:=shape(dimensions);var unit:=minf(1.0,dimensions.y/1.28)
	var inner:=shape(dimensions,.060*unit)
	skin.get_node("SideWalls").mesh=mesh_surface(outer,outer,-.26*unit,.10*unit)
	skin.get_node("RearShell").mesh=mesh_surface(outer,outer,-.26*unit,-.26*unit,true,true)
	skin.get_node("BevelRing").mesh=mesh_surface(outer,inner,.10*unit,FRONT_Z*unit)
	skin.get_node("GlassFront").mesh=mesh_surface(inner,inner,FRONT_Z*unit,FRONT_Z*unit,true)
	title_label.width=(dimensions.x-.96*unit)/title_label.pixel_size
	glyph_label.position.x=-dimensions.x*.5+.32*unit

func set_selected(value: bool, animate_flow: bool=false) -> void:
	chosen=value;motion=animate_flow
	if not is_instance_valid(bevel_material):return
	bevel_material.set_shader_parameter("selected",1.0 if value else 0.0)
	bevel_material.set_shader_parameter("accent",accent)
	bevel_material.set_shader_parameter("motion",value and animate_flow)
	_apply_context_tone()

func pose(hover: bool, pressed: bool=false, pointer: Vector2=Vector2.ZERO, reduced: bool=false) -> void:
	if effect and effect.is_valid():effect.kill()
	var released:=was_pressed and not pressed
	was_pressed=pressed
	bevel_material.set_shader_parameter("hovered",1.0 if hover and not pressed else 0.0)
	var lift:=0.22 if chosen else 0.0
	if pressed:lift-=0.12
	elif hover:lift+=0.22
	var angle:=Vector3(0,-.025,0)
	if hover:angle+=Vector3(-pointer.y*.025,pointer.x*.035,0)
	if pressed:angle*=.35
	if reduced:
		skin.position.z=lift;skin.rotation=angle
	else:
		effect=create_tween().set_parallel(true).set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
		effect.tween_property(skin,"position:z",lift,0.20 if released else 0.15).set_trans(Tween.TRANS_BACK if released else Tween.TRANS_CUBIC)
		effect.tween_property(skin,"rotation",angle,0.15)

func animating() -> bool:
	return effect != null and effect.is_valid() and effect.is_running()

func set_context_dim(value: bool) -> void:
	set_context_strength(1.0 if value else 0.0)

func set_context_strength(value: float) -> void:
	if is_equal_approx(context_strength,value):return
	context_strength=clampf(value,0,1);context_dim=context_strength>.5
	_apply_context_tone()

func _apply_context_tone() -> void:
	var amount:=context_strength
	var level:=lerpf(1.0,.32,amount)
	face_material.albedo_color=(Color.WHITE.lerp(accent,.09) if chosen else Color("d2dbe0"))*Color(level,level,level,1)
	face_material.roughness=lerpf(.30,.82,amount)
	face_material.metallic_specular=lerpf(.18,.06,amount)
	face_material.clearcoat=lerpf(.30,.05,amount)
	if visual_theme != null:
		face_material.albedo_color*=visual_theme.surface_tint
		face_material.roughness=lerpf(visual_theme.surface_roughness,.82,amount)
		face_material.clearcoat=lerpf(visual_theme.clearcoat,.05,amount)
	bevel_material.set_shader_parameter("context_fade",amount)
	var side: StandardMaterial3D=skin.get_node("SideWalls").material_override
	side.albedo_color=Color("496273").lerp(accent,.12)*Color(level,level,level,1);side.roughness=lerpf(.30,.8,amount)
	if visual_theme != null:
		side.albedo_color*=visual_theme.metal_tint
		side.roughness=lerpf(visual_theme.surface_roughness,.8,amount)
	glyph_label.set_color(accent*Color(level,level,level,1))
	title_label.modulate=theme_text(not context_dim)

func theme_text(active: bool=true) -> Color:
	if visual_theme != null and visual_theme.id != "default":return visual_theme.text if active else visual_theme.muted
	return Color("e1eeee") if active else Color("9cb5bf")

func apply_visual_theme(value: Resource) -> void:
	# Source-context text dimming belongs to the consuming view, independently
	# of material context_fade. Retain that role when exchanging only tokens.
	var muted_text:=is_instance_valid(title_label) and title_label.modulate.is_equal_approx(theme_text(false))
	visual_theme=value
	if not is_instance_valid(face_material):return
	bevel_material.set_shader_parameter("theme_metal_tint",value.metal_tint)
	bevel_material.set_shader_parameter("theme_roughness",value.surface_roughness-.30)
	_apply_context_tone()
	if muted_text:title_label.modulate=theme_text(false)

func set_navigation_role(value: float, animate_flow: bool) -> void:
	bevel_material.set_shader_parameter("navigation_role",value)
	bevel_material.set_shader_parameter("navigation_motion",animate_flow)
	face_material.emission_enabled=not is_zero_approx(value)
	face_material.emission=Color("e9b96e") if value<0 else Color("7ce6ef")
	face_material.emission_energy_multiplier=.08
