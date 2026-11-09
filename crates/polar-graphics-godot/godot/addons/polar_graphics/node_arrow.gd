extends Button

## A real Button retains native focus/press/capture semantics. The glyph and
## state marker are drawn within the existing card gutter, never below it.
var direction:="children"
var expansion_state:="collapsed"
var _hover:=0.0
var _hover_tween:Tween
func _ready()->void:
	mouse_filter=Control.MOUSE_FILTER_STOP
	for state in ["font_color","font_hover_color","font_pressed_color","font_disabled_color"]:add_theme_color_override(state,Color.TRANSPARENT)
	for state in ["normal","hover","pressed","disabled","focus"]:add_theme_stylebox_override(state,StyleBoxEmpty.new())
	mouse_entered.connect(func():_fade(1.0))
	mouse_exited.connect(func():_fade(0.0))
func _fade(value:float)->void:
	if _hover_tween!=null:_hover_tween.kill()
	var preferences=get_parent().get("motion_preferences")
	if preferences is Dictionary and bool(preferences.get("reduce_motion",false)):
		_hover=value;queue_redraw();return
	_hover_tween=create_tween();_hover_tween.tween_method(func(progress:float):_hover=progress;queue_redraw(),_hover,value,0.1)
func _draw()->void:
	var color:=Color("a3afbf") if disabled else Color("d8e6f3").lerp(Color("4de0cc"),_hover)
	var center:=size*0.5
	if _hover>0:
		var style:=StyleBoxFlat.new();style.bg_color=Color(0.3,0.8,0.75,_hover*0.17);style.set_corner_radius_all(5);draw_style_box(style,Rect2(Vector2.ONE,size-Vector2.ONE*2))
	var sign_value:=1.0 if direction=="children" else -1.0
	draw_polyline(PackedVector2Array([center+Vector2(-4,-2*sign_value),center+Vector2(0,2*sign_value),center+Vector2(4,-2*sign_value)]),color,1.7,true)
	var marker:=center+Vector2(8,0)
	match expansion_state:
		"none":draw_circle(marker,1.0,Color(color,0.35))
		"collapsed":draw_circle(marker,1.5,color)
		"partial":draw_circle(marker+Vector2(0,-2),1.1,color);draw_circle(marker+Vector2(0,2),1.1,color)
		"full":draw_line(marker+Vector2(0,-3),marker+Vector2(0,3),color,1.4,true)
