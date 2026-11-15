extends RefCounted
## One composition for the existing spatial and flat detail surfaces. Native
## adapters consume these logical Pane rectangles without a second layout rule.
static func compose(area: Vector2, preferred_detail: Vector2) -> Dictionary:
	var margin := 16.0 if area.x >= 680 else 8.0
	var gap := 12.0
	var room := (area - Vector2.ONE * margin * 2).max(Vector2.ONE)
	var detail: Rect2
	var editor: Rect2
	if area.x >= 680:
		var ew := clampf(area.x * .24, 220, 340)
		var dw := minf(preferred_detail.x, room.x - ew - gap)
		var dh := minf(preferred_detail.y, room.y)
		var eh := minf(maxf(180, dh), minf(320, room.y))
		var x := (area.x - dw - ew - gap) * .5
		var y := clampf((area.y - dh) * .5 + 45, margin, area.y - margin - maxf(dh, eh))
		detail = Rect2(x, y, dw, dh)
		editor = Rect2(x + dw + gap, y, ew, eh)
	else:
		var eh := minf(maxf(140, room.y * .45), maxf(1, room.y - gap - 64))
		var dh := maxf(1, minf(preferred_detail.y, room.y - gap - eh))
		var y := maxf(margin, (area.y - dh - eh - gap) * .5)
		detail = Rect2(margin, y, room.x, dh)
		editor = Rect2(margin, y + dh + gap, room.x, eh)
	return {"detail": detail, "editor": editor}
