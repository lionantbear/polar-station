extends Button

const Surface = preload("res://addons/polar_graphics/depth/spatial_wheel_plaque.gdshader")
signal grip_started(card, event)
var accent := Color("5cd9ce")
var chosen := false
var source_index := -1
var glyph := 0
var title := ""
var subtitle := ""
var source_card := false
var face: ColorRect
var words: Label
var note: Label
var grip: Button

func _ready() -> void:
	mouse_default_cursor_shape = Control.CURSOR_POINTING_HAND
	focus_mode = Control.FOCUS_NONE
	for state in ["normal", "hover", "pressed", "focus"]:
		add_theme_stylebox_override(state, StyleBoxEmpty.new())
	face = ColorRect.new()
	face.mouse_filter = Control.MOUSE_FILTER_IGNORE
	face.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	var surface := ShaderMaterial.new()
	surface.shader = Surface
	face.material = surface
	add_child(face)
	words = Label.new()
	words.mouse_filter = Control.MOUSE_FILTER_IGNORE
	words.add_theme_color_override("font_color", Color("e5eeef"))
	words.add_theme_font_size_override("font_size", 16 if source_card else 18)
	words.autowrap_mode = TextServer.AUTOWRAP_WORD_SMART
	words.vertical_alignment = VERTICAL_ALIGNMENT_CENTER
	add_child(words)
	note = Label.new()
	note.mouse_filter = Control.MOUSE_FILTER_IGNORE
	note.add_theme_color_override("font_color", Color("8ca5ae"))
	note.add_theme_font_size_override("font_size", 11)
	add_child(note)
	if source_card:
		grip = Button.new()
		grip.text = "⠿"
		grip.tooltip_text = "Drag to reorder"
		grip.add_theme_font_size_override("font_size", 18)
		grip.add_theme_color_override("font_color", Color("7eaaa9"))
		for state in ["normal", "hover", "pressed", "focus"]:
			grip.add_theme_stylebox_override(state, StyleBoxEmpty.new())
		grip.mouse_default_cursor_shape = Control.CURSOR_DRAG
		grip.gui_input.connect(func(event): grip_started.emit(self, event))
		add_child(grip)
		grip.modulate.a = 0.0
	mouse_entered.connect(func():
		face.material.set_shader_parameter("hover_amount", 1.0)
		if grip: grip.modulate.a = 1.0
		queue_redraw())
	mouse_exited.connect(func():
		face.material.set_shader_parameter("hover_amount", 0.0)
		if grip: grip.modulate.a = 0.0
		queue_redraw())
	resized.connect(refresh)
	refresh()

func refresh() -> void:
	if not is_instance_valid(face): return
	face.material.set_shader_parameter("card_size", size)
	face.material.set_shader_parameter("accent", accent)
	face.material.set_shader_parameter("emphasis", 1.0 if chosen else 0.0)
	words.text = title
	words.position = Vector2(39 if source_card else 46, 8)
	words.size = Vector2(size.x - (59 if source_card else 70), size.y - (20 if subtitle.is_empty() else 43))
	note.text = subtitle
	note.position = Vector2(46, size.y-25)
	note.size = Vector2(size.x-66, 17)
	if grip:
		grip.position = Vector2(size.x-26, size.y*0.5-13)
		grip.size = Vector2(20,26)
	queue_redraw()

func _draw() -> void:
	var polygon := PackedVector2Array([Vector2(0,size.y*0.5),Vector2(9,0),Vector2(size.x-10,0),Vector2(size.x,size.y*0.5),Vector2(size.x-10,size.y),Vector2(9,size.y)])
	var shadow := Transform2D(0, Vector2(1,7))*polygon
	draw_colored_polygon(shadow, Color(0.0,0.015,0.025,0.6))
	# Shallow lower metal edge, never an opaque track behind the sources.
	draw_polyline(PackedVector2Array([Vector2(4,size.y*0.5+4),Vector2(12,size.y+4),Vector2(size.x-10,size.y+4),Vector2(size.x,size.y*0.5+3)]), Color(accent,0.28 if chosen else 0.10),2,true)

func _notification(what: int) -> void:
	if what == NOTIFICATION_DRAW and is_instance_valid(face):
		# Glyph is drawn above the face by a separate small child (see below).
		pass
