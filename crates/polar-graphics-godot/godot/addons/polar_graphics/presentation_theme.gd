extends RefCounted

## Consumer-independent visual tokens. Semantic source/type assignments belong
## to the application's projection; these resources only draw their encoding.
const BACKGROUND := Color("151a22")
const CARD := Color("252d38")
const TEXT := Color("e8edf4")
const MUTED := Color("a3afbf")
const SELECTED := Color("4de0cc")
const SOURCES := [Color("73aef4"), Color("bd8de0"), Color("d2b879"), Color("78c5ac"), Color("e79aa8"), Color("82cadb")]
const HOVER_SECONDS := 0.1
const TRANSITION_SECONDS := 0.22

static func source_color(ordinal: int) -> Color:
	return SOURCES[posmod(ordinal, SOURCES.size())]

static func panel(accent: Color = CARD) -> StyleBoxFlat:
	var result := StyleBoxFlat.new()
	result.bg_color = CARD
	result.border_color = accent
	result.set_border_width_all(1)
	result.set_corner_radius_all(10)
	result.set_content_margin_all(12)
	return result

static func controls() -> Theme:
	var result := Theme.new()
	result.default_font_size = 14
	for kind in ["Label", "Button", "RichTextLabel", "CheckButton"]:
		result.set_color("font_color", kind, TEXT)
	result.set_stylebox("panel", "PanelContainer", panel())
	for state in ["normal","hover","pressed","disabled","focus"]:
		var style:=panel(CARD.lightened(0.15) if state=="normal" else SELECTED.darkened(0.3))
		style.content_margin_left=10;style.content_margin_right=10;style.content_margin_top=7;style.content_margin_bottom=7
		if state=="focus":style.bg_color=Color.TRANSPARENT
		if state=="disabled":style.border_color=CARD;style.bg_color=CARD.darkened(0.1)
		result.set_stylebox(state,"Button",style)
	result.set_color("font_disabled_color","Button",MUTED)
	return result
