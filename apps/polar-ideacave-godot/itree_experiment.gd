extends Control
## Thin standalone host. All browsing, themes and persistence use the PS scene.
var workspace
func _ready() -> void:
	_configure_window()
	get_window().dpi_changed.connect(_configure_window)
	var column := VBoxContainer.new()
	column.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	column.add_theme_constant_override("separation", 0)
	add_child(column)
	workspace = preload("res://itree_workspace.tscn").instantiate()
	workspace.set_meta("view_id", get_meta("view_id", "standalone-experiment"))
	workspace.set_meta("native_controls", true)
	workspace.size_flags_vertical = Control.SIZE_EXPAND_FILL
	column.add_child(workspace)
	var footer := HBoxContainer.new()
	column.add_child(footer)
	var views := OptionButton.new()
	views.add_item("Mind Map"); views.add_item("Depth Forest")
	views.selected = 1 if workspace.browser_id == "depth_forest" else 0
	views.item_selected.connect(func(index): workspace.host_command("browser_depth_forest" if index == 1 else "browser_mind_map"))
	footer.add_child(views)
	var themes := OptionButton.new()
	themes.add_item("Default"); themes.add_item("Christmas")
	themes.selected = 1 if workspace.theme_id == "christmas" else 0
	themes.item_selected.connect(func(index): workspace.host_command("theme_christmas" if index == 1 else "theme_default"))
	footer.add_child(themes)
	var open_files := Button.new()
	open_files.text = "Open files"
	open_files.pressed.connect(workspace._choose_files)
	footer.add_child(open_files)

func _configure_window() -> void:
	var window := get_window()
	window.content_scale_size = Vector2i.ZERO
	window.content_scale_mode = Window.CONTENT_SCALE_MODE_CANVAS_ITEMS
	window.content_scale_factor = maxf(1.0, DisplayServer.screen_get_scale(window.current_screen))
