extends Control
var accent := Color("5cd9ce")
var kind := 0
func _ready() -> void:
	mouse_filter = Control.MOUSE_FILTER_IGNORE
func _draw() -> void:
	var c := size*0.5
	var r := 9.0
	match kind % 4:
		0:
			for y in [-5.0,1.0,7.0]:
				draw_polyline(PackedVector2Array([c+Vector2(-r,y),c+Vector2(0,y+5),c+Vector2(r,y),c+Vector2(0,y-5),c+Vector2(-r,y)]),accent,1.4,true)
		1:
			draw_arc(c,r,0,TAU,32,accent,1.4,true)
			draw_circle(c,2.3,accent)
			for v in [Vector2.UP,Vector2.DOWN,Vector2.LEFT,Vector2.RIGHT]: draw_line(c+v*7,c+v*13,accent,1.4,true)
		2:
			for point in [c+Vector2(0,-9),c+Vector2(-9,8),c+Vector2(9,8)]:
				draw_line(c,point,accent,1.4,true)
				draw_arc(point,3.5,0,TAU,16,accent,1.4,true)
		3:
			draw_polyline(PackedVector2Array([c+Vector2(-5,-9),c+Vector2(-12,0),c+Vector2(-5,9)]),accent,1.6,true)
			draw_polyline(PackedVector2Array([c+Vector2(5,-9),c+Vector2(12,0),c+Vector2(5,9)]),accent,1.6,true)

## Shared original glyph strokes, also consumed by the native 3D plaque skin.
static func strokes(value: int) -> Array[PackedVector2Array]:
	var result: Array[PackedVector2Array]=[]
	match value%4:
		0:
			for y in [-5.0,1.0,7.0]:result.append(PackedVector2Array([Vector2(-9,y),Vector2(0,y+5),Vector2(9,y),Vector2(0,y-5),Vector2(-9,y)]))
		1:
			result.append(_circle(Vector2.ZERO,9,40));result.append(_circle(Vector2.ZERO,2,14))
			for v in [Vector2.UP,Vector2.DOWN,Vector2.LEFT,Vector2.RIGHT]:result.append(PackedVector2Array([v*7,v*13]))
		2:
			for p in [Vector2(0,-9),Vector2(-9,8),Vector2(9,8)]:
				result.append(PackedVector2Array([Vector2.ZERO,p]));result.append(_circle(p,3.5,18))
		3:
			result.append(PackedVector2Array([Vector2(-5,-9),Vector2(-12,0),Vector2(-5,9)]))
			result.append(PackedVector2Array([Vector2(5,-9),Vector2(12,0),Vector2(5,9)]))
	return result

static func _circle(center: Vector2,radius: float,steps: int) -> PackedVector2Array:
	var points:=PackedVector2Array()
	for i in range(steps+1):points.append(center+Vector2(cos(TAU*i/steps),sin(TAU*i/steps))*radius)
	return points
