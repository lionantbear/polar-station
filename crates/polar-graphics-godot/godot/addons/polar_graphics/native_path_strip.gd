extends MultiMeshInstance3D
## Persistent native quads for a world-space path. The existing flow material
## consumes per-instance distance, length and total length through INSTANCE_CUSTOM.
var _path:=PackedVector3Array()
var _width:=-1.0
var build_count:=0

func _init()->void:
	var quad=ResourceLoader.get_cached_ref("res://.polar-graphics/path-quad-v1") as QuadMesh
	if quad==null:
		quad=QuadMesh.new();quad.size=Vector2.ONE
		quad.take_over_path("res://.polar-graphics/path-quad-v1")
	multimesh=MultiMesh.new();multimesh.transform_format=MultiMesh.TRANSFORM_3D
	multimesh.use_custom_data=true;multimesh.mesh=quad
	cast_shadow=GeometryInstance3D.SHADOW_CASTING_SETTING_OFF

func sync(path:PackedVector3Array,width:float)->void:
	if path==_path and is_equal_approx(width,_width):return
	_path=path;_width=width;build_count+=1
	var segments:Array=[];var total:=0.0
	for i in range(1,path.size()):
		var direction:=path[i]-path[i-1];var length:=direction.length()
		if length<.00001:continue
		var tangent:=direction/length
		var normal:=tangent.cross(Vector3.BACK)
		if normal.length_squared()<.00001:normal=tangent.cross(Vector3.UP)
		normal=normal.normalized()
		segments.append([Transform3D(Basis(tangent*length,normal*width,tangent.cross(normal)),(path[i]+path[i-1])*.5),total,length])
		total+=length
	if multimesh.instance_count!=segments.size():multimesh.instance_count=segments.size()
	for i in segments.size():
		multimesh.set_instance_transform(i,segments[i][0])
		multimesh.set_instance_custom_data(i,Color(segments[i][1],segments[i][2],maxf(total,.001),0))
