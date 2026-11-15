extends Control
## Static border decoration, separate from data stars and relationship marks.
## No input, timers, sound, animation or processing while a Pane is hidden.
var visual_theme: Resource

func _ready() -> void:
	mouse_filter = Control.MOUSE_FILTER_IGNORE
	focus_mode = Control.FOCUS_NONE
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	resized.connect(queue_redraw)
	set_process(false)

func apply_visual_theme(value: Resource) -> void:
	visual_theme = value
	visible = value.trim_enabled
	queue_redraw()

func _draw() -> void:
	if visual_theme == null or not visual_theme.trim_enabled:return
	# Only the outer 14 logical pixels: do not cover labels, controls or cards.
	for side in [-1.0, 1.0]:
		var origin := Vector2(12 if side > 0 else size.x - 12, 7)
		for i in 6:
			var stem := origin + Vector2(side * i * 12, 0)
			draw_line(stem, stem + Vector2(side * 13, 0), visual_theme.trim_pine, 1, true)
			draw_line(stem, stem + Vector2(side * 6, 5), visual_theme.trim_pine, 1, true)
			draw_line(stem, stem + Vector2(side * 6, -4), visual_theme.trim_pine, 1, true)
			if i % 2 == 0:
				draw_circle(stem + Vector2(side * 6, 2), 2.2, visual_theme.trim_light)
			else:draw_circle(stem + Vector2(side * 7, 3), 1.8, visual_theme.trim_berry)
