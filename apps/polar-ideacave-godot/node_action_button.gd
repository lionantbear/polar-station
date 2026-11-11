extends RefCounted
## Static vector artwork and native Button styling shared by node/detail rails.
const PATHS:={
	"jump":"M5 19L19 5M8 5H19V16",
	"read":"M5 4H19V20H5ZM8 9H16M8 13H16M8 17H13",
	"edit":"M5 16L16 5L20 9L9 20H5ZM14 7L18 11",
	"close":"M6 6L18 18M18 6L6 18"
}
static func create(kind:String,tip:String)->Button:
	var button:=Button.new()
	button.name=kind.capitalize()+"Action"
	button.focus_mode=Control.FOCUS_NONE
	button.custom_minimum_size=Vector2(30,30)
	button.mouse_default_cursor_shape=Control.CURSOR_POINTING_HAND
	button.tooltip_text=tip
	var key:="res://.polar-graphics/node-action/"+kind
	var texture:=ResourceLoader.get_cached_ref(key) as Texture2D
	if texture==null:
		var image:=Image.new()
		image.load_svg_from_string('<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24"><path d="'+PATHS[kind]+'" fill="none" stroke="#ddf5f3" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round"/></svg>')
		texture=ImageTexture.create_from_image(image);texture.take_over_path(key)
	button.icon=texture
	for state in ["normal","hover","pressed","focus"]:
		var style:=StyleBoxFlat.new()
		style.bg_color=Color("12313c") if state=="normal" else Color("245360")
		style.bg_color.a=.86
		style.border_color=Color("51747e") if state=="normal" else Color("9be4df")
		style.set_border_width_all(1);style.set_corner_radius_all(5)
		style.set_content_margin_all(3)
		button.add_theme_stylebox_override(state,style)
	return button
