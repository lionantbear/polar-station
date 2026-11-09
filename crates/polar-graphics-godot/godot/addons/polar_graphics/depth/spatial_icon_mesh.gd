extends MeshInstance3D
const Glyph=preload("res://addons/polar_graphics/depth/spatial_wheel_glyph.gd")
var kind:=0
var tint:=Color("65d8ce")
var icon_material: StandardMaterial3D
func _ready() -> void:
	var vertices:=PackedVector3Array();var normals:=PackedVector3Array()
	for stroke in Glyph.strokes(kind):
		for i in range(1,stroke.size()):
			var a:=stroke[i-1]/26.0;var b:=stroke[i]/26.0
			var n: Vector2=(b-a).normalized().orthogonal()*.023
			var points: Array[Vector3]=[]
			for p in [a-n,b-n,b+n,a-n,b+n,a+n]:points.append(Vector3(p.x,-p.y,0))
			for j in [0,3]:
				var one: Vector3=points[j];var two: Vector3=points[j+1];var three: Vector3=points[j+2]
				# Godot's Vector2.orthogonal is clockwise; the Y inversion into
				# plaque coordinates must still yield a clockwise +Z front.
				if (two-one).cross(three-one).z>0:
					var swap:=two;two=three;three=swap
				for point in [one,two,three]:vertices.append(point);normals.append(Vector3.BACK)

	var arrays: Array=[];arrays.resize(Mesh.ARRAY_MAX);arrays[Mesh.ARRAY_VERTEX]=vertices;arrays[Mesh.ARRAY_NORMAL]=normals
	var geometry:=ArrayMesh.new();geometry.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES,arrays);mesh=geometry
	icon_material=StandardMaterial3D.new();icon_material.shading_mode=BaseMaterial3D.SHADING_MODE_UNSHADED
	# Shared strokes become clockwise, front-facing engraving quads.
	icon_material.cull_mode=BaseMaterial3D.CULL_BACK;material_override=icon_material
	set_color(tint)
func set_color(value: Color) -> void:
	tint=value
	if icon_material:icon_material.albedo_color=tint
