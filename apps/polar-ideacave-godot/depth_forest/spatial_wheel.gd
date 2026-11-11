extends Control

## IT-02B visual layer only. Source identity, relation eligibility, occurrence
## paths and Graphviz routing remain owned by the existing preview modules.
const ViewModel = preload("res://itree_view_model.gd")
const Navigation = preload("res://itree_navigation_state.gd")
const SourceWheel = preload("res://tree_wheel.gd")
const TreeProjection = preload("res://itree_tree_projection.gd")
const Plaque = preload("res://addons/polar_graphics/depth/spatial_wheel_plaque.gd")
const Glyph = preload("res://addons/polar_graphics/depth/spatial_wheel_glyph.gd")
const ACCENTS = [Color("d4b071"),Color("58dcd0"),Color("d0aa77"),Color("bda0e1")]
var prepared_model
var prepared_navigation
var model = ViewModel.new()
var navigation = Navigation.new()
var sources: Array[Dictionary] = []
var plaques: Array[Button] = []
var cards: Dictionary = {}
var screen_positions: Dictionary = {}
var layout: Dictionary = {}
var layouts: Dictionary = {}
var active_index := 1
var wheel_value := 1.0
var wheel_width := 158.0
var min_wheel_width:=125.0
var transition: Tween
var tree_pan := Vector2.ZERO
var tree_zoom := 1.0
var tree_origin := Vector2.ZERO
var tree_scale := 1.0
var dragging_canvas := false
var dragging_source := -1
var resizing_wheel := false
var initialized := false
var error := ""
var heading: Label
var context_label: Label
var footer: Label
var reader: PanelContainer
var reader_text: RichTextLabel
var source_tooltip: Label
var width_grip: Control
var pan_accumulator := 0.0
var wheel_accumulator := 0.0
var last_node_click := ""
var last_node_time := 0

func _ready() -> void:
	if get_parent() == get_tree().root:
		get_window().content_scale_size = Vector2i.ZERO
		get_window().content_scale_mode = Window.CONTENT_SCALE_MODE_DISABLED
	set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT)
	mouse_filter = Control.MOUSE_FILTER_PASS
	if prepared_model == null or prepared_navigation == null:
		error = "Depth Forest requires the owning Pane's loaded preview model."
		push_error(error)
		return
	if not _read_sources():return
	_build_chrome()
	_build_sources()
	select_source(active_index,false)
	resized.connect(_resize)
	_resize()
	initialized = true
	_trace_state()
	print("SPATIAL_WHEEL_READY sources=%d nodes=%d backend=%s display=%s" % [sources.size(),model.records.size(),layout.get("backend",""),DisplayServer.get_name()])

func _read_sources()->bool:
	sources.clear()
	model = prepared_model
	navigation = prepared_navigation
	for root in model.source_roots:
		var ordinal := sources.size()
		sources.append({"id":root,"title":str(model.records[root].title),"accent":ACCENTS[ordinal % ACCENTS.size()],"glyph":ordinal % 4})
	if sources.is_empty():
		error = "No loaded source roots."
		return false
	var entries: Array=sources.map(func(source):return {"key":source.id,"title":source.title})
	var short_titles: Dictionary=SourceWheel._display_labels(entries)
	for source in sources:source.title=short_titles[source.id]
	active_index = 0
	for i in sources.size():
		if model.records[sources[i].id].source == model.records[navigation.selected_id].source:active_index=i
	wheel_value = active_index
	return true

func _label(value: String, font_size: int, color: Color) -> Label:
	var label := Label.new()
	label.text = value
	label.add_theme_font_size_override("font_size",font_size)
	label.add_theme_color_override("font_color",color)
	label.mouse_filter = Control.MOUSE_FILTER_IGNORE
	add_child(label)
	return label

func _build_chrome() -> void:
	heading = _label("IDEA TREE   /   SPATIAL WHEEL",14,Color("a4c5ce"))
	heading.position = Vector2(29,23)
	context_label = _label("",13,Color("728e9c"))
	footer = _label("Wheel  Change tree     ·     Right + wheel  Zoom     ·     Right-drag  Pan     ·     Enter  Details",12,Color("728d9c"))
	source_tooltip = _label("",14,Color("dce8ea"))
	source_tooltip.z_index = 100
	var tooltip_style := StyleBoxFlat.new()
	tooltip_style.bg_color = Color("182b35")
	tooltip_style.set_content_margin_all(10)
	source_tooltip.add_theme_stylebox_override("normal",tooltip_style)
	source_tooltip.hide()
	width_grip = Control.new()
	width_grip.mouse_default_cursor_shape = Control.CURSOR_HSIZE
	width_grip.tooltip_text = "Drag to resize the source wheel"
	width_grip.gui_input.connect(func(event):
		if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT:
			resizing_wheel = event.pressed
			accept_event())
	add_child(width_grip)
	reader = PanelContainer.new()
	var style := StyleBoxFlat.new()
	style.bg_color = Color("12232d")
	style.border_color = Color("46706f")
	style.set_border_width_all(1)
	style.set_corner_radius_all(9)
	style.set_content_margin_all(20)
	reader.add_theme_stylebox_override("panel",style)
	reader.visible = false
	add_child(reader)
	var column := VBoxContainer.new()
	reader.add_child(column)
	var close := Button.new()
	close.text = "Close details   ·   Esc"
	close.pressed.connect(func(): reader.hide())
	column.add_child(close)
	reader_text = RichTextLabel.new()
	reader_text.add_theme_font_size_override("normal_font_size",16)
	reader_text.size_flags_vertical = Control.SIZE_EXPAND_FILL
	reader_text.bbcode_enabled = false
	column.add_child(reader_text)

func _build_sources() -> void:
	for card in plaques: card.queue_free()
	plaques.clear()
	for i in sources.size():
		var source: Dictionary = sources[i]
		var card = Plaque.new()
		card.source_card = true
		card.title = source.title
		card.accent = source.accent
		card.glyph = source.glyph
		card.source_index = i
		card.mouse_entered.connect(func():
			source_tooltip.text = str(model.records[source.id].title)
			source_tooltip.position = card.position+Vector2(card.size.x+23,12)
			source_tooltip.show())
		card.mouse_exited.connect(func(): source_tooltip.hide())
		add_child(card)
		_add_glyph(card,source.glyph,source.accent,true)
		card.pressed.connect(func(): select_source(sources.find(source)))
		card.grip_started.connect(_grip_input)
		plaques.append(card)
	_place_sources()

func _add_glyph(card: Control, kind: int, accent: Color, small: bool) -> void:
	var icon = Glyph.new()
	icon.kind = kind
	icon.accent = accent
	icon.position = Vector2(10 if small else 14,0)
	icon.size = Vector2(26,78 if small else 72)
	card.add_child(icon)

func _place_sources() -> void:
	var center_y := size.y*0.48
	var radius := clampf(size.y*0.40,180,325)
	for i in plaques.size():
		var card = plaques[i]
		var distance := float(i)-wheel_value
		var angle := clampf(distance*0.55,-1.60,1.60)
		var depth := cos(angle)
		card.visible = absf(distance) < 3.0
		card.size = Vector2(wheel_width,78)
		# Front-facing labels keep full vertical proportions. Depth changes the
		# whole plaque uniformly, never squeezes type into a disabled-looking row.
		card.scale = Vector2.ONE*(0.91+0.09*maxf(0,depth))
		card.position = Vector2(22+18*depth,center_y+sin(angle)*radius-39)
		card.rotation = distance*0.012
		card.z_index = 20+int(depth*10)
		card.chosen = i == active_index
		card.refresh()
	width_grip.position = Vector2(wheel_width+51,90)
	width_grip.size = Vector2(8,size.y-145)
	queue_redraw()
	_trace_state()

func select_source(index: int, animate: bool = true) -> void:
	index = clampi(index,0,sources.size()-1)
	var changed := active_index != index or layout.is_empty()
	print("SPATIAL_WHEEL_SELECT index=%d animate=%s" % [index,animate])
	active_index = index
	if transition and transition.is_valid(): transition.kill()
	if animate:
		transition = create_tween()
		transition.set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_OUT)
		transition.finished.connect(_trace_state)
		transition.tween_method(func(value: float):
			wheel_value = value
			_place_sources(),wheel_value,float(index),0.36)
	else:
		wheel_value = index
		_place_sources()
	if changed:
		var root := str(sources[index].id)
		# Preserve each tree's remembered exact occurrence when switching.
		var memory := str(sources[index].get("selection",root))
		navigation.select_node(memory)
		tree_pan = Vector2.ZERO
		tree_zoom = 1.0
		_rebuild_tree()
	else:
		_refresh_selection()

func step_source(delta: int) -> void:
	select_source(active_index+delta)
	_trace_state()

func wheel_step(delta: int, factor: float = 1.0) -> void:
	# Discrete events represent one detent regardless of acceleration factor.
	# Fractional standalone mouse input has one explicit accumulated threshold.
	var amount := float(delta)*minf(absf(factor),1.0)
	if wheel_accumulator*amount<0: wheel_accumulator=0.0
	if (active_index==0 and amount<0) or (active_index==sources.size()-1 and amount>0):
		wheel_accumulator=0.0
		return
	wheel_accumulator+=amount
	if absf(wheel_accumulator)>=0.99999:
		step_source(1 if wheel_accumulator>0 else -1)
		wheel_accumulator=0.0

func needs_continuous_frames() -> bool:
	return is_visible_in_tree() and transition != null and transition.is_valid() and transition.is_running()

func _trace_state() -> void:
	var path := OS.get_environment("POLAR_LAB_STATE_PATH")
	if path.is_empty() or sources.is_empty(): return
	var state := {"ready": initialized,"active_index": active_index,"wheel_value": wheel_value,
		"active_source": sources[active_index].id,"selected_id": navigation.selected_id,
		"zoom": tree_zoom,"pan": [tree_pan.x,tree_pan.y],"continuous_frames": needs_continuous_frames(),
		"size": [size.x,size.y],"sources": [],"nodes": [],"reader_visible": reader.visible if is_instance_valid(reader) else false}
	for i in plaques.size():
		var card = plaques[i]
		var rectangle: Rect2 = card.get_global_rect()
		state.sources.append({"index": i,"id": sources[i].id,"point": [rectangle.get_center().x,rectangle.get_center().y],"visible": card.visible})
	for key in cards:
		var rectangle: Rect2 = cards[key].get_global_rect()
		state.nodes.append({"key": key,"id": layout.items[key].node_id,"point": [rectangle.get_center().x,rectangle.get_center().y]})
	var file := FileAccess.open(path,FileAccess.WRITE)
	if file: file.store_string(JSON.stringify(state))

func _rebuild_tree() -> void:
	var root := str(sources[active_index].id)
	layout = TreeProjection.build(model,[root],navigation.expanded,120,"horizontal")
	# The visual skin supplies its real bounds back to the existing router.
	# Routing at the old compact height would run cards into one another.
	if str(layout.error).is_empty():
		for key in layout.sizes:
			layout.sizes[key] = Vector2(maxf(240,layout.sizes[key].x+28),96)
		var geometry: Dictionary = TreeProjection._graphviz(layout.items.keys(),layout.edges,layout.sizes,"dot","LR")
		layout.error = geometry.error
		layout.positions = geometry.positions
		layout.routes = geometry.routes
		layout.bounds = TreeProjection._bounds(layout.positions,layout.sizes,layout.routes)
	if not str(layout.error).is_empty():
		error = str(layout.error)
		push_error(error)
		return
	for card in cards.values(): card.queue_free()
	cards.clear()
	for key in layout.items:
		var item: Dictionary = layout.items[key]
		var card = Plaque.new()
		card.title = str(item.title)
		card.accent = sources[active_index].accent if int(item.depth) == 0 else Color("9bb8bc")

		card.subtitle = "SOURCE ROOT" if int(item.depth) == 0 else str(item.get("slot",""))
		add_child(card)
		_add_glyph(card,int(sources[active_index].glyph) if int(item.depth) == 0 else int(item.branch)%4,card.accent,false)
		card.gui_input.connect(func(event): _node_input(key,event))
		card.pressed.connect(func(): _select_node(key))
		cards[key] = card
	_refresh_selection()
	_place_tree()

func _refresh_selection() -> void:
	for key in cards:
		cards[key].chosen = str(layout.items[key].node_id) == navigation.selected_id
		cards[key].refresh()
	if is_instance_valid(context_label):
		var root := str(sources[active_index].id)
		var unresolved := 0
		for edge in model.relation_edges:
			if str(edge.from) == root and not bool(edge.resolved): unresolved += 1
		context_label.text = "LOCAL DOCUMENT PREVIEW   /   %s   /   %d visible nodes%s" % [str(model.records[root].title),cards.size(),"   /   %d unloaded references" % unresolved if unresolved else ""]
	queue_redraw()

func _place_tree() -> void:
	if layout.is_empty() or layout.positions.is_empty(): return
	var area := Rect2(Vector2(wheel_width+126,120),Vector2(maxf(320,size.x-wheel_width-205),maxf(300,size.y-245)))
	var bounds: Rect2 = layout.bounds
	# Existing Graphviz routing is retained. A 1.25x card construction scale
	# provides the reading-sized plaque skin without fitting text every frame.
	tree_scale = minf(minf(area.size.x/bounds.size.x,area.size.y/bounds.size.y),1.20)*tree_zoom
	tree_scale = maxf(tree_scale,0.62)
	tree_origin = area.get_center()-bounds.get_center()*tree_scale+tree_pan
	screen_positions.clear()
	for key in layout.items:
		var card = cards[key]
		var center: Vector2 = tree_origin+layout.positions[key]*tree_scale
		var dimensions: Vector2 = layout.sizes[key]*tree_scale
		dimensions.x = maxf(dimensions.x,200)
		dimensions.y = maxf(dimensions.y,76)
		card.position = center-dimensions*0.5
		card.size = dimensions
		card.z_index = 10
		card.refresh()
		screen_positions[key] = center
	queue_redraw()
	_trace_state()

func _resize() -> void:
	if not is_instance_valid(footer): return
	context_label.position = Vector2(wheel_width+125,61)
	footer.position = Vector2(29,size.y-33)
	reader.position = Vector2(maxf(wheel_width+70,size.x-540),100)
	reader.size = Vector2(minf(500,size.x-wheel_width-110),size.y-160)
	reader.z_index = 80
	_place_sources()
	_place_tree()

func _draw() -> void:
	draw_rect(Rect2(Vector2.ZERO,size),Color("091923"))
	# One continuous information plane spans selector and canvas. Grid lines
	# remain behind all cards and never terminate at a selector boundary.
	for i in range(-12,24):
		var y := float(i)*72
		draw_line(Vector2(0,y),Vector2(size.x,y-size.x*0.20),Color(0.20,0.39,0.46,0.11),1,true)
	for i in range(-15,25):
		var x := float(i)*108
		draw_line(Vector2(x,0),Vector2(x+size.y*0.72,size.y),Color(0.20,0.39,0.46,0.10),1,true)
	for i in range(0,20):
		for j in range(0,12):
			var point := Vector2(i*108+j*22,j*72-i*12)
			if Rect2(Vector2.ZERO,size).has_point(point): draw_circle(point,0.9,Color(0.52,0.72,0.72,0.16))
	if not layout.is_empty():
		for route in layout.get("routes",[]):
			var points := PackedVector2Array()
			for p in route.points: points.append(tree_origin+p*tree_scale)
			if points.size()<2: continue
			var accent: Color = sources[active_index].accent
			draw_polyline(points,Color(accent,0.06),6,true)
			draw_polyline(points,Color(accent,0.62),1.35,true)
			var end := points[-1]
			var direction := (end-points[-2]).normalized()
			var side := direction.orthogonal()
			draw_colored_polygon(PackedVector2Array([end,end-direction*8+side*3.5,end-direction*8-side*3.5]),Color(accent,0.9))
	if plaques.size()>active_index:
		var active = plaques[active_index]
		var point: Vector2 = active.position+Vector2(active.size.x+8,active.size.y*0.5)
		draw_colored_polygon(PackedVector2Array([point+Vector2(0,-6),point+Vector2(5,0),point+Vector2(0,6),point+Vector2(-5,0)]),sources[active_index].accent)

func _select_node(key: String) -> void:
	navigation.select_occurrence(key,layout)
	sources[active_index]["selection"] = navigation.selected_id
	_refresh_selection()

func _node_input(key: String, event: InputEvent) -> void:
	if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT and event.pressed and event.double_click:
		_select_node(key)
		show_details()
		accept_event()

func show_details() -> void:
	var record: Dictionary = model.records[navigation.selected_id]
	var parts: Array[String] = [str(record.title),"",str(record.description),"","Source: "+str(record.source),"SHA-256: "+str(record.source_sha256),"Key: "+str(record.key),"","Relations (original slots):"]
	for edge in model.relation_edges:
		if str(edge.from) == navigation.selected_id:
			parts.append(str(edge.slot)+"  →  "+str(model.records.get(str(edge.to),{}).get("title",edge.to))+("  [unloaded]" if not bool(edge.resolved) else ""))
	reader_text.text = "\n".join(parts)
	reader.show()

func _grip_input(card, event: InputEvent) -> void:
	if event is InputEventMouseButton and event.button_index == MOUSE_BUTTON_LEFT:
		dragging_source = plaques.find(card) if event.pressed else -1
		accept_event()

func reorder_source(from: int, to: int) -> void:
	if from == to or from<0 or to<0 or from>=sources.size() or to>=sources.size(): return
	var active_id := str(sources[active_index].id)
	var moved: Dictionary = sources.pop_at(from)
	sources.insert(to,moved)
	for i in sources.size():
		if str(sources[i].id) == active_id: active_index = i
	if transition and transition.is_valid(): transition.kill()
	wheel_value = active_index
	_build_sources()

func _input(event: InputEvent) -> void:
	if not initialized: return
	if event is InputEventMouseMotion:
		if resizing_wheel:
			wheel_width = clampf(wheel_width+event.relative.x,min_wheel_width,260)
			_resize()
			accept_event()
		elif dragging_source>=0:
			var closest := dragging_source
			var distance := INF
			for i in plaques.size():
				var d := absf(event.position.y-plaques[i].get_global_rect().get_center().y)
				if d<distance:
					distance=d
					closest=i
			if closest != dragging_source:
				reorder_source(dragging_source,closest)
				dragging_source=closest
			accept_event()
		elif dragging_canvas:
			tree_pan += event.relative
			_place_tree()
			accept_event()
	if event is InputEventMouseButton:
		if not event.pressed and event.button_index == MOUSE_BUTTON_LEFT:
			dragging_source=-1
			resizing_wheel=false
		if event.button_index == MOUSE_BUTTON_RIGHT:
			dragging_canvas=event.pressed
			accept_event()
		if event.pressed and event.button_index in [MOUSE_BUTTON_WHEEL_UP,MOUSE_BUTTON_WHEEL_DOWN]:
			var delta := 1 if event.button_index == MOUSE_BUTTON_WHEEL_DOWN else -1
			if (event.button_mask & MOUSE_BUTTON_MASK_RIGHT) != 0:
				_zoom_at(event.position,pow(1.12,-delta))
			else: wheel_step(delta,event.factor)
			accept_event()
	if event is InputEventPanGesture:
		if event.position.x<wheel_width+65 or event.alt_pressed:
			pan_accumulator += event.delta.y
			if absf(pan_accumulator)>=1.0:
				step_source(1 if pan_accumulator>0 else -1)
				pan_accumulator=0
		else:
			tree_pan -= event.delta*22
			_place_tree()
		accept_event()
	if event is InputEventMagnifyGesture:
		_zoom_at(event.position,event.factor)
		accept_event()
	if event is InputEventKey and event.pressed and not event.echo:
		if event.keycode == KEY_ESCAPE: reader.hide()
		elif event.keycode == KEY_ENTER: show_details()
		elif event.keycode in [KEY_UP,KEY_DOWN,KEY_LEFT,KEY_RIGHT,KEY_SPACE]:
			_keyboard_node(event.keycode)
			accept_event()

func _zoom_at(point: Vector2, factor: float) -> void:
	var before := tree_origin
	var previous := tree_scale
	tree_zoom = clampf(tree_zoom*factor,0.6,2.8)
	_place_tree()
	if previous>0:
		tree_pan += point-(tree_origin+(point-before)*(tree_scale/previous))
	_place_tree()

func _keyboard_node(keycode: Key) -> void:
	var selected: String = navigation.selected_occurrence
	if not layout.items.has(selected): return
	var item: Dictionary = layout.items[selected]
	if keycode == KEY_SPACE:
		navigation.toggle_occurrence(selected,layout)
		_rebuild_tree()
	elif keycode == KEY_LEFT:
		if not str(item.parent).is_empty(): _select_node(str(item.parent))
	elif keycode == KEY_RIGHT:
		navigation.expand_occurrence(selected,layout)
		_rebuild_tree()
		for edge in layout.edges:
			if str(edge.from) == selected:
				_select_node(str(edge.to))
				break
	else:
		var siblings: Array = []
		for candidate in layout.items:
			if str(layout.items[candidate].parent) == str(item.parent): siblings.append(candidate)
		var index := siblings.find(selected)
		var target := clampi(index+(1 if keycode == KEY_DOWN else -1),0,siblings.size()-1)
		if not siblings.is_empty(): _select_node(str(siblings[target]))
