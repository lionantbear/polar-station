extends RefCounted
## Presentation control handles only. Godot owns curve tessellation and gradient
## interpolation; callers retain these world paths with their layout resource.
static var _fade:Gradient

static func build(start:Vector3,end:Vector3,side:float,width:float)->Curve3D:
	var depth:=end.z-start.z
	var exit:=Vector3(side*clampf(width*.14,.4,1.2),0,0)
	var curve:=Curve3D.new()
	curve.up_vector_enabled=false
	curve.bake_interval=maxf(.25,start.distance_to(end)/96.0)
	var a:=start+exit;var b:=end-exit
	curve.add_point(start,Vector3.ZERO,exit/3.0)
	if absf(depth)<.001:
		var handle:=Vector3(side*minf(2.0,start.distance_to(end)*.25),0,0)
		curve.add_point(a,-exit/3.0,handle)
		curve.add_point(b,-handle,exit/3.0)
	else:
		var radius:=minf(1.5,minf(absf(depth)*.12,width*.25))
		var across:=Vector3(side*radius,0,0)
		var into:=Vector3(0,0,signf(depth)*radius)
		var tangent:=Vector3(0,0,depth*.28)
		curve.add_point(a,-exit/3.0,across*.55228475)
		curve.add_point(a+across+into,-into*.55228475,tangent)
		curve.add_point(b-across-into,-tangent,into*.55228475)
		curve.add_point(b,-across*.55228475,exit/3.0)
	curve.add_point(end,-exit/3.0,Vector3.ZERO)
	return curve

static func alpha(points:PackedVector3Array)->PackedFloat32Array:
	if _fade==null:
		_fade=Gradient.new()
		_fade.offsets=PackedFloat32Array([0,.12,.35,.65,.88,1])
		_fade.colors=PackedColorArray([Color(1,1,1,1),Color(1,1,1,.8),Color(1,1,1,.22),Color(1,1,1,.22),Color(1,1,1,.8),Color(1,1,1,1)])
	var weights:=PackedFloat32Array()
	var depth:=points[-1].z-points[0].z
	for i in points.size():
		var progress:float=(points[i].z-points[0].z)/depth if absf(depth)>.001 else float(i)/maxi(1,points.size()-1)
		weights.append(_fade.sample(clampf(progress,0,1)).a)
	return weights

static func takeoff(start:Vector3,outward:Vector3,depth:float,run:float)->Curve3D:
	var curve:=Curve3D.new();curve.bake_interval=.2
	var into:=Vector3(0,0,signf(depth));var distance:=absf(depth)
	if run<.001:
		curve.add_point(start,Vector3.ZERO,into*distance/3)
		curve.add_point(start+into*distance,-into*distance/3,Vector3.ZERO)
		return curve
	var radius:=minf(.28,minf(run*.65,distance*.25));var straight:=run-radius
	curve.add_point(start,Vector3.ZERO,outward*straight/3)
	curve.add_point(start+outward*straight,-outward*straight/3,outward*radius*.55228475)
	curve.add_point(start+outward*run+into*radius,-into*radius*.55228475,into*(distance-radius)/3)
	curve.add_point(start+outward*run+into*distance,-into*(distance-radius)/3,Vector3.ZERO)
	return curve

static func depth_join(start:Vector3,end:Vector3)->Curve3D:
	# Both tangents stay in Z: convergence happens after the local takeoff,
	# without another horizontal hook or an artificial upward arch.
	var curve:=Curve3D.new();curve.bake_interval=.25
	var handle:=Vector3(0,0,(end.z-start.z)*.45)
	curve.add_point(start,Vector3.ZERO,handle);curve.add_point(end,-handle,Vector3.ZERO)
	return curve

static func departure_alpha(points:PackedVector3Array)->PackedFloat32Array:
	var fade:=Gradient.new();fade.offsets=PackedFloat32Array([0,.25,1])
	fade.colors=PackedColorArray([Color.WHITE,Color(1,1,1,.5),Color(1,1,1,.22)])
	var result:=PackedFloat32Array();var depth:=points[-1].z-points[0].z
	for point in points:result.append(fade.sample(clampf((point.z-points[0].z)/depth,0,1)).a)
	return result
