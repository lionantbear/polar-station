extends RefCounted

## Cached extrusion of presentation shapes. Caller supplies semantic shape names;
## no business types or ranking are inferred here. All shapes occupy similar bounds.
static var _cache:Dictionary={}
static var _outlines:Dictionary={}
static var _selection_cache:Dictionary={}
static func mesh(shape:String)->Mesh:
	if _cache.has(shape):return _cache[shape]
	var outline:=PackedVector2Array()
	match shape:
		"diamond":outline=PackedVector2Array([Vector2(0,1),Vector2(1,0),Vector2(0,-1),Vector2(-1,0)])
		"hexagon":
			for i in 6:outline.append(Vector2(cos(TAU*i/6.0),sin(TAU*i/6.0)))
		"capsule":
			for side in 2:
				for i in 13:
					var angle:=-PI*0.5+PI*i/12.0+PI*side
					outline.append(Vector2(0.5 if side==0 else -0.5,0)+Vector2(cos(angle),sin(angle))*0.55)
		"rounded_block":
			var radius:=0.24
			for corner in 4:
				var angle:=PI*0.5*corner
				var center:=Vector2(cos(angle+PI*0.25),sin(angle+PI*0.25))*(1.0-radius)*sqrt(2.0)
				for i in 5:
					var a:=angle+PI*0.5*i/4.0
					outline.append(center+Vector2(cos(a),sin(a))*radius)
		"double_plate", "placeholder":outline=PackedVector2Array([Vector2(-1,-0.75),Vector2(1,-0.75),Vector2(1,0.75),Vector2(-1,0.75)])
		_:
			for i in 16:outline.append(Vector2(cos(TAU*i/16.0),sin(TAU*i/16.0)))
	_outlines[shape]=outline
	var tool:=SurfaceTool.new();tool.begin(Mesh.PRIMITIVE_TRIANGLES)
	var triangles:=Geometry2D.triangulate_polygon(outline)
	var layers:Array=[Vector2(-0.28,0.28)]
	if shape=="double_plate":layers=[Vector2(-0.42,-0.18),Vector2(0.18,0.42)]
	for layer:Vector2 in layers:
		for i in range(0,triangles.size(),3):
			for index in [i,i+1,i+2]:
				var p:Vector2=outline[triangles[index]];tool.add_vertex(Vector3(p.x,p.y,layer.y))
			for index in [i+2,i+1,i]:
				var p:Vector2=outline[triangles[index]];tool.add_vertex(Vector3(p.x,p.y,layer.x))
		for i in outline.size():
			var a:Vector2=outline[i];var b:Vector2=outline[(i+1)%outline.size()]
			for p in [Vector3(a.x,a.y,layer.x),Vector3(b.x,b.y,layer.x),Vector3(b.x,b.y,layer.y),Vector3(a.x,a.y,layer.x),Vector3(b.x,b.y,layer.y),Vector3(a.x,a.y,layer.y)]:tool.add_vertex(p)
	tool.generate_normals()
	var result:=tool.commit()
	_cache[shape]=result
	return result

static func selection(shape:String)->Mesh:
	if _selection_cache.has(shape):return _selection_cache[shape]
	mesh(shape)
	var outline:PackedVector2Array=_outlines[shape]
	var vertices:=PackedVector3Array()
	for z_value in [-0.45,0.45]:
		for i in outline.size():
			var a:Vector2=outline[i];var b:Vector2=outline[(i+1)%outline.size()]
			vertices.append(Vector3(a.x,a.y,z_value));vertices.append(Vector3(b.x,b.y,z_value))
	var arrays:Array=[];arrays.resize(Mesh.ARRAY_MAX);arrays[Mesh.ARRAY_VERTEX]=vertices
	var result:=ArrayMesh.new();result.add_surface_from_arrays(Mesh.PRIMITIVE_LINES,arrays)
	_selection_cache[shape]=result;return result
