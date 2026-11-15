extends Resource
## Immutable package input, duplicated for each consumer before application.
## Tokens have no Idea Tree identity, camera, layout, font-size or audio state.
@export var id := "default"
@export var version := 1
@export var title := "Default"
@export var background := Color("151a22")
@export var card := Color("252d38")
@export var text := Color("e8edf4")
@export var muted := Color("a3afbf")
@export var selection := Color("4de0cc")
@export var border := Color("526475")
@export var ambient := Color("aec7d7")
@export var key_light := Color.WHITE
@export var fill_light := Color("94d6e0")
@export var sky_top := Color("172939")
@export var sky_horizon := Color("9eafb6")
@export var surface_tint := Color.WHITE
@export var metal_tint := Color.WHITE
@export var surface_roughness := 0.30
@export var clearcoat := 0.30
@export var trim_enabled := false
@export var trim_pine := Color("355c4c")
@export var trim_light := Color("f5d899")
@export var trim_berry := Color("b5585b")
const PACKAGES := {"default":"res://addons/polar_graphics/themes/default.tres", "christmas":"res://addons/polar_graphics/themes/christmas.tres"}

static func load_theme(theme_id: String, resource_path: String = "") -> Resource:
	var path:String=resource_path if not resource_path.is_empty() else PACKAGES.get(theme_id,"")
	if path.is_empty() or not ResourceLoader.exists(path):return null
	var resource=load(path)
	if resource==null or not resource.has_method("valid") or not resource.valid():return null
	return resource.duplicate(true)

func valid() -> bool:
	return not id.is_empty() and version == 1 and not title.is_empty() and is_finite(surface_roughness) and surface_roughness >= 0 and surface_roughness <= 1 and is_finite(clearcoat) and clearcoat >= 0 and clearcoat <= 1

func controls(baseline: Theme) -> Theme:
	# Fonts retain their original resources/shaping caches. Only style boxes
	# and colors are copied below; deep-copying SystemFont invalidates glyph data.
	var result := baseline.duplicate() as Theme if baseline != null else Theme.new()
	# Preserve font resources, sizes, constants, spacing and hit geometry.
	for kind in ["Label", "Button", "RichTextLabel", "CheckButton", "LineEdit", "TextEdit", "OptionButton", "PopupMenu", "ItemList"]:
		result.set_color("font_color", kind, text)
		result.set_color("font_hover_color", kind, text)
		result.set_color("font_pressed_color", kind, text)
		result.set_color("font_disabled_color", kind, muted)
		result.set_color("font_focus_color", kind, text)
		result.set_color("selection_color", kind, Color(selection, .25))
	for kind in result.get_type_list():
		for name in result.get_stylebox_list(kind):
			var original := result.get_stylebox(name, kind)
			if original is StyleBoxFlat:result.set_stylebox(name, kind, panel_style(original))
	return result

func panel_style(baseline: StyleBoxFlat) -> StyleBoxFlat:
	var result := baseline.duplicate() as StyleBoxFlat
	if baseline.bg_color.a > 0 and baseline.bg_color.get_luminance() < .35:
		result.bg_color = Color(card, baseline.bg_color.a)
	if baseline.border_color.s < .35:result.border_color = Color(border, baseline.border_color.a)
	elif baseline.border_color.is_equal_approx(Color("4de0cc")):result.border_color = selection
	return result
