extends Node3D
## A mode-local reading surface using the existing exact-node model and controls.
const InstructionLayout=preload("res://node_instruction_layout.gd")
const Plaque=preload("res://addons/polar_graphics/depth/spatial_plaque_mesh.gd")
const DIMENSIONS:=Vector2(10.6,12.4)
var app
var forest
var viewport: SubViewport
var departing_surface: MeshInstance3D
var surface: MeshInstance3D
var shell
var active:=false
var focus_id:=""
var context_ids: Array=[]
var base_points: Dictionary={}
var base_bounds: Dictionary={}
var base_edges: Array=[]
var base_atlas_points:Dictionary={}
var base_cache: Dictionary={}
var effect: Tween
const PIXELS_PER_UNIT:=44.0
var dimensions:=DIMENSIONS
var face_size:=Vector2(10.1,11.8)
var base_cards: Dictionary={}
var roles: Dictionary={"parents":[],"children":[]}
var offsets: Dictionary={"parents":0,"children":0}
var shown: Dictionary={"parents":[],"children":[]}
var peers: Dictionary={}
var regions: Dictionary={}
var instruction_open:=false
var instruction_editor_rect:=Rect2()
var instruction_button: Button
var close_button:Button
const ActionButton=preload("res://node_action_button.gd")
var annotations: Control
var headings: Dictionary={}
var counters: Dictionary={}
var pagers: Dictionary={}
var tags: Dictionary={}
var role_anchors: Dictionary={}
var counter_anchors: Dictionary={}
var role_references: Dictionary={}
var goals: Dictionary={}
var compact_wheel:=false
var reader_center:=Vector2.ZERO
var group_width:=0.0
var scroll_accumulator: Dictionary={"parents":0.0,"children":0.0}
var tooltip: Label
var saved_direction:=Vector3.ZERO
var arrival_from:=""
var band_ids: Dictionary={"parents":[],"level":[],"children":[]}
var layout_key:=""
var packed_points: Dictionary={}
var packed_bounds: Dictionary={}
var band_x: Dictionary={}
var packed_bands: Dictionary={}
var reader_x:=0.0
var layout_origin:=Vector3.ZERO
var detail_zoom:=1.0
var context_density:=1.0
var font_delta:=0
var focus_visible: Dictionary={}
var full_peers: Array=[]
var manual_offset:=Vector3.ZERO
var requested_reader_size:=Vector2.ZERO
var drag_kind:=""
var last_navigation_ms: int = -1000
var attachment: MeshInstance3D
var attachment_material: StandardMaterial3D
var move_hint: Label
var resize_hint: Label

func setup(owner_app) -> void:
	app=owner_app;forest=app.forest
	viewport=SubViewport.new();viewport.size=Vector2i(720,840);viewport.transparent_bg=true
	viewport.render_target_update_mode=SubViewport.UPDATE_DISABLED;add_child(viewport)
	app.reader.reparent(viewport);app.reader.position=Vector2.ZERO;app.reader.size=Vector2(viewport.size)
	app.reader.theme=app.theme
	for font_name in ["normal_font","bold_font","italics_font","bold_italics_font","mono_font"]:app.reader_text.add_theme_font_override(font_name,ThemeDB.fallback_font)
	var panel:=StyleBoxFlat.new();panel.bg_color=Color(0,0,0,0);panel.set_content_margin_all(24);panel.content_margin_right=68
	app.reader.add_theme_stylebox_override("panel",panel)
	app.reader_text.add_theme_font_size_override("normal_font_size",36)
	app.reader_text.add_theme_font_size_override("bold_font_size",36)
	app.reader_text.bbcode_enabled=true;app.reader_text.selection_enabled=true
	var column: VBoxContainer=app.reader.get_child(0)
	var close: Button=column.get_child(0)
	for connection in close.pressed.get_connections():close.pressed.disconnect(connection.callable)
	close.hide()
	shell=Plaque.new();shell.dimensions=DIMENSIONS;shell.title="";add_child(shell)
	shell.title_label.hide();shell.glyph_label.hide();shell.skin.rotation=Vector3.ZERO
	shell.set_selected(true,true)
	surface=MeshInstance3D.new();var mesh:=QuadMesh.new();mesh.size=Vector2(10.1,11.8);surface.mesh=mesh
	var material:=StandardMaterial3D.new();material.shading_mode=BaseMaterial3D.SHADING_MODE_UNSHADED
	material.transparency=BaseMaterial3D.TRANSPARENCY_ALPHA;material.albedo_texture=viewport.get_texture();material.cull_mode=BaseMaterial3D.CULL_DISABLED
	surface.material_override=material;surface.position.z=.24;add_child(surface);hide()
	departing_surface=MeshInstance3D.new();departing_surface.mesh=QuadMesh.new();departing_surface.material_override=material;forest.world.add_child(departing_surface);departing_surface.hide()
	annotations=Control.new();annotations.mouse_filter=Control.MOUSE_FILTER_IGNORE;annotations.z_index=12;forest.add_child(annotations);annotations.hide()
	instruction_button=ActionButton.create("edit","Edit instruction (Enter)")
	instruction_button.pressed.connect(func():
		if app.has_signal("instruction_requested"):app.instruction_requested.emit())
	annotations.add_child(instruction_button)
	instruction_button.visible=app.has_signal("instruction_requested")
	close_button=ActionButton.create("close","Return to tree (Esc)")
	close_button.pressed.connect(app._close_details);annotations.add_child(close_button)
	for role in ["parents","children","previous","next"]:
		var heading:=Label.new();heading.add_theme_font_size_override("font_size",18);heading.modulate=Color("afccd5");heading.mouse_filter=Control.MOUSE_FILTER_IGNORE;annotations.add_child(heading);headings[role]=heading
		if role in ["parents","children"]:
			var counter:=Label.new();counter.add_theme_font_size_override("font_size",15);counter.modulate=Color("91aeb9");counter.mouse_filter=Control.MOUSE_FILTER_IGNORE;annotations.add_child(counter);counters[role]=counter
			var buttons: Array=[]
			for direction in [-1,1]:
				var button:=Button.new();button.text="↑" if direction<0 else "↓";button.focus_mode=Control.FOCUS_NONE;button.custom_minimum_size=Vector2(34,30)
				button.add_theme_font_size_override("font_size",20);button.tooltip_text="Previous labels" if direction<0 else "More labels"
				button.pressed.connect(func():scroll_group(role,direction));annotations.add_child(button);buttons.append(button)
			pagers[role]=buttons
	tooltip=Label.new();tooltip.autowrap_mode=TextServer.AUTOWRAP_WORD_SMART;tooltip.add_theme_font_size_override("font_size",20);tooltip.mouse_filter=Control.MOUSE_FILTER_IGNORE
	var tip_style:=StyleBoxFlat.new();tip_style.bg_color=Color("102b38");tip_style.set_content_margin_all(12);tip_style.set_border_width_all(1);tip_style.border_color=Color("6d9faa")
	tooltip.add_theme_stylebox_override("normal",tip_style);annotations.add_child(tooltip);tooltip.hide()
	move_hint=Label.new();move_hint.text="Drag to move   ·   Pinch / right-wheel to zoom";move_hint.add_theme_font_size_override("font_size",14);move_hint.modulate=Color("94b6c4");move_hint.mouse_filter=Control.MOUSE_FILTER_IGNORE;annotations.add_child(move_hint)
	resize_hint=Label.new();resize_hint.text="◢";resize_hint.add_theme_font_size_override("font_size",21);resize_hint.modulate=Color("83c4cd");resize_hint.mouse_filter=Control.MOUSE_FILTER_IGNORE;annotations.add_child(resize_hint)
	attachment=MeshInstance3D.new();forest.world.add_child(attachment);attachment.hide()
	attachment_material=StandardMaterial3D.new();attachment_material.shading_mode=BaseMaterial3D.SHADING_MODE_UNSHADED;attachment_material.albedo_color=Color("73c7d4");attachment_material.no_depth_test=false;attachment.material_override=attachment_material

func _escape(value: String) -> String:
	return value.replace("[","[lb]")

func _property_text(value) -> String:
	if value is Array:return "\n".join(value.map(func(item):return _property_text(item)))
	if value is Dictionary:
		var lines: Array[String]=[]
		for key in value:
			var text:=_property_text(value[key])
			if str(key) in ["git_revision","sha256"]:text=text.left(12)+"…"
			lines.append(str(key).capitalize()+": "+text)
		return "\n".join(lines)
	return str(value)

var _rendered_id := ""

func _content(record: Dictionary) -> void:
	var saved_scroll: float=app.reader_text.get_v_scroll_bar().value if _rendered_id==focus_id else 0.0
	_rendered_id=focus_id
	var title:=_escape(str(record.title));var description:=str(record.get("description",""))
	var source_title:=str(app.sources[app.active_index].title)
	var parents: int=app.model.parents.get(focus_id,[]).size();var children: int=app.model.children.get(focus_id,[]).size()
	var counts: String="%d %s · %d %s"%[parents,"parent" if parents==1 else "parents",children,"child" if children==1 else "children"]
	var parts: Array[String]=["[color=#7adbd3]CURRENT NODE · DETAILS[/color]","[font_size="+str(int((32+font_delta)*1.5))+"][b]"+title+"[/b][/font_size]","[color=#91aeb9]"+source_title+" · "+counts+"[/color]",""]
	if bool(record.get("external",false)):
		parts=["[color=#7adbd3]CURRENT NODE · DETAILS[/color]","[font_size=40]Unloaded reference[/font_size]","This original reference endpoint is present, but its contents are not loaded.","UUID: "+_escape(str(record.get("uuid","")))]
	else:
		if not description.is_empty() and description!=str(record.title):parts.append(_escape(description)+"\n")
		for key in record.get("properties",{}):
			if key in ["name","title","statement","text","summary","description","goal","sources"]:continue
			parts.append("[b]"+_escape(str(key).capitalize())+"[/b]\n"+_escape(_property_text(record.properties[key]))+"\n")
		if record.get("properties",{}).has("sources"):
			parts.append("[b]Source references[/b]\n"+_escape(_property_text(record.properties.sources)))
	parts.append("\n[color=#91aeb9]Browse nearby labels or use arrows.\n↑↓ same level   ← parent   → child[/color]")
	app.reader_text.text="\n".join(parts);app.reader_text.get_v_scroll_bar().set_deferred("value",saved_scroll)
	app.reader.position=Vector2.ZERO;app.reader.size=Vector2(viewport.size);app.reader.show()
	viewport.render_target_update_mode=SubViewport.UPDATE_ONCE

func _restore_geometry() -> void:
	if base_points.is_empty():return
	forest.points=base_points.duplicate();forest.bounds=base_bounds.duplicate();forest.edge_paths=base_edges.duplicate(true);forest._route_cache=base_cache.duplicate(true)
	forest.visible_nodes.clear()
	for id in forest.plaques_3d:
		var card=forest.plaques_3d[id]
		card.position=forest.points[id];card.visible=true;card.scale=Vector3.ONE;card.skin.scale=Vector3.ONE;card.title_label.scale=Vector3.ONE;card.title_label.modulate.a=1;forest.visible_nodes[id]=true
		# The selected reader also morphs its hidden plaque, but need not have
		# a context-label cache. Restore every mesh independently of that cache.
		card.resize_geometry(base_bounds[id])
		if base_cards.has(id):
			var state: Dictionary=base_cards[id]
			card.title_label.text=state.text;card.title_label.position=state.position;card.title_label.pixel_size=state.pixel_size;card.title_label.width=state.width
			card.glyph_label.show()
	forest.star_atlas.world_points=base_atlas_points

func _shared_parent(id: String) -> bool:
	for parent in app.model.parents.get(id,[]):
		if parent in app.model.parents.get(focus_id,[]):return true
	return false

func _source_title(id: String) -> String:
	var source: String=forest.source_by_node.get(id,"")
	for item in app.sources:
		if str(item.id)==source:return str(item.title)
	return "Unloaded reference"

func _caption(id: String,peer: bool=false) -> String:
	var text: String=("Shared parent" if _shared_parent(id) else "Different parent") if peer else ""
	if forest.source_by_node.get(id)!=forest.source_by_node.get(focus_id):
		text+=(" · " if not text.is_empty() else "")+_source_title(id)
	return text

func _fit_title(value: String,width: float,lines: float=3.05) -> String:
	var font:=ThemeDB.fallback_font
	var flags:=TextServer.BREAK_MANDATORY|TextServer.BREAK_WORD_BOUND|TextServer.BREAK_ADAPTIVE
	var max_height:=font.get_height(64)*lines
	if font.get_multiline_string_size(value,HORIZONTAL_ALIGNMENT_CENTER,width,64,-1,flags).y<=max_height:return value
	var low:=0;var high:=value.length()
	while low<high:
		var middle: int=(low+high+1)/2
		if font.get_multiline_string_size(value.left(middle)+"…",HORIZONTAL_ALIGNMENT_CENTER,width,64,-1,flags).y<=max_height:low=middle
		else:high=middle-1
	return value.left(low)+"…"

func _context_size(id: String,width_px: float) -> Vector2:
	var card=forest.plaques_3d[id]
	if not base_cards.has(id):base_cards[id]={"text":card.title_label.text,"position":card.title_label.position,"pixel_size":card.title_label.pixel_size,"width":card.title_label.width}
	var measured_width: float=ThemeDB.fallback_font.get_string_size(str(forest.record_for(id).title),HORIZONTAL_ALIGNMENT_CENTER,-1,64).x*20.0/64.0+24
	var width:=clampf(measured_width,110,width_px)/PIXELS_PER_UNIT
	card.title_label.pixel_size=20.0/(64.0*PIXELS_PER_UNIT)
	var text_width: float=(width-.45)/card.title_label.pixel_size
	var title:=_fit_title(str(forest.record_for(id).title),text_width)
	var flags:=TextServer.BREAK_MANDATORY|TextServer.BREAK_WORD_BOUND|TextServer.BREAK_ADAPTIVE
	var measured:=ThemeDB.fallback_font.get_multiline_string_size(title,HORIZONTAL_ALIGNMENT_CENTER,text_width,64,-1,flags)
	var height: float=maxf(1.28,measured.y*card.title_label.pixel_size+.42)
	card.resize_geometry(Vector2(width,height));card.title_label.width=text_width;card.title_label.text=title;card.title_label.position.x=0;card.glyph_label.hide()
	return Vector2(width,height)

func _tag(id: String,text: String) -> void:
	if not tags.has(id):
		var label:=Label.new();label.add_theme_font_size_override("font_size",14);label.modulate=Color("a9c7cf");label.mouse_filter=Control.MOUSE_FILTER_IGNORE;annotations.add_child(label);tags[id]=label
	tags[id].text=text;tags[id].visible=not text.is_empty()

func _siblings(id: String) -> Array:
	var result: Array=[]
	for parent in app.model.parents.get(id,[]):
		for edge in forest.visual_children(str(parent)):
			var key:=str(edge.to)
			if key!=id and key not in result:result.append(key)
	return result

func _family(seeds: Array) -> Array:
	var result: Array=seeds.duplicate()
	for id in seeds:
		for sibling in _siblings(str(id)):
			if sibling not in result:result.append(sibling)
	return result

func _ordered(ids: Array) -> Array:
	var result: Array=ids.filter(func(id):return base_points.has(id))
	result.sort_custom(func(a,b):return base_points[a].y>base_points[b].y if not is_equal_approx(base_points[a].y,base_points[b].y) else str(a)<str(b))
	return result

func _collect_bands() -> void:
	roles={"parents":[],"children":[]}
	for id in app.model.parents.get(focus_id,[]):
		if base_points.has(id):roles.parents.append(id)
	for edge in forest.visual_children(focus_id):
		if str(edge.to) not in roles.children:roles.children.append(str(edge.to))
	var level: Array=forest.visual_level(focus_id)
	for id in _siblings(focus_id):
		if id not in level:level.append(id)
	if focus_id not in level:level.append(focus_id)
	band_ids={"parents":_ordered(_family(roles.parents)),"level":_ordered(level),"children":_ordered(_family(roles.children))}
	context_ids=[];peers.clear()
	for role in ["level","parents","children"]:
		band_ids[role]=band_ids[role].filter(func(id):return id not in context_ids)
		context_ids.append_array(band_ids[role])
	var at: int=band_ids.level.find(focus_id)
	if at>0:peers.previous=band_ids.level[at-1]
	if at+1<band_ids.level.size():peers.next=band_ids.level[at+1]
	shown={"parents":band_ids.parents.duplicate(),"children":band_ids.children.duplicate()}

func _reader_dimensions(value: Vector2) -> void:
	dimensions=value/PIXELS_PER_UNIT;face_size=dimensions-Vector2(.35,.35)
	shell.resize_geometry(dimensions);surface.mesh.size=face_size
	viewport.size=Vector2i(roundi(face_size.x*PIXELS_PER_UNIT*1.5),roundi(face_size.y*PIXELS_PER_UNIT*1.5))
	app.reader.position=Vector2.ZERO;app.reader.size=Vector2(viewport.size)
	viewport.render_target_update_mode=SubViewport.UPDATE_ONCE

func _focus_card(id: String,screen: Vector2,pixels: Vector2,full: bool=false) -> void:
	var card=forest.plaques_3d[id]
	if not base_cards.has(id):base_cards[id]={"text":card.title_label.text,"position":card.title_label.position,"pixel_size":card.title_label.pixel_size,"width":card.title_label.width}
	var font_px:=maxf(18,20+font_delta)
	card.title_label.pixel_size=font_px/(64.0*PIXELS_PER_UNIT)
	var bounds:=pixels/PIXELS_PER_UNIT
	card.resize_geometry(bounds)
	card.title_label.width=(pixels.x-24)/font_px*64
	card.title_label.text=_fit_title(str(forest.record_for(id).title),card.title_label.width,maxf(1.05,(pixels.y-12)/(font_px*1.2)) if not full else 3.05)
	card.title_label.position.x=0;card.glyph_label.hide()
	forest.bounds[id]=bounds;packed_bounds[id]=bounds
	goals[id]=layout_origin+Vector3((screen.x-forest.size.x*.5)/PIXELS_PER_UNIT,(forest.size.y*.5-screen.y)/PIXELS_PER_UNIT,0)
	var rectangle:=Rect2(screen-pixels*.5,pixels)
	var content:=Rect2(12,142,maxf(1,forest.size.x-24),maxf(1,forest.size.y-192))
	if content.encloses(rectangle):focus_visible[id]=true

func _build_context() -> bool:
	_collect_bands();compact_wheel=true;detail_zoom=1.0
	var key: String=str(forest.source_by_node.get(focus_id))+":"+str(snappedf(base_points[focus_id].x,.1))+":"+str(forest.size)
	var fresh:=key!=layout_key
	if fresh:layout_key=key;layout_origin=base_points[focus_id];offsets={"parents":0,"children":0}
	var w: float=forest.size.x;var h: float=forest.size.y
	var rw:=minf(960,w*.56);var rh:=maxf(200,(h-160)*.49)
	reader_x=w*.5;reader_center=Vector2(w*.5,h*.5+45)
	instruction_editor_rect=Rect2()
	if instruction_open:
		var pair:Dictionary=InstructionLayout.compose(forest.size,Vector2(rw,rh))
		var detail:Rect2=pair.detail
		rw=detail.size.x;rh=detail.size.y;reader_center=detail.get_center();reader_x=reader_center.x
		instruction_editor_rect=pair.editor
	_reader_dimensions(Vector2(rw,rh))
	band_x={"parents":(w-rw)*.25,"level":w*.5,"children":w-(w-rw)*.25}
	context_ids=[];goals.clear();focus_visible.clear();full_peers.clear()
	for role in ["level","parents","children"]:
		band_ids[role]=band_ids[role].filter(func(id):return id not in context_ids)
		context_ids.append_array(band_ids[role])
	for id in context_ids:
		goals[id]=layout_origin+Vector3(0,1000,0)
		forest.bounds[id]=base_bounds[id]
	goals[focus_id]=layout_origin+Vector3((reader_center.x-w*.5)/PIXELS_PER_UNIT,(h*.5-reader_center.y)/PIXELS_PER_UNIT,0);forest.bounds[focus_id]=dimensions
	var edge_width:=maxf(60,(w-rw)*.5-90)
	var edge_widths:Dictionary={"parents":edge_width,"children":edge_width}
	if instruction_open:
		# The native editor moves the reader off centre. Context stays outside
		# their combined footprint, rather than overlapping the shifted reader.
		var left_room:=reader_center.x-rw*.5
		var right_begin:=maxf(reader_center.x+rw*.5,instruction_editor_rect.end.x)
		band_x.parents=left_room*.5;band_x.children=(right_begin+w)*.5
		edge_widths={"parents":maxf(60,left_room-28),"children":maxf(60,w-right_begin-28)}
	for role in ["parents","children"]:
		var y:=150.0;var step:=maxf(66,72*context_density+font_delta*2)
		for index in range(int(offsets[role]),band_ids[role].size()):
			if y+step>h-55:break
			var id: String=band_ids[role][index]
			_focus_card(id,Vector2(band_x[role],y+step*.5),Vector2(edge_widths[role],step-18));y+=step
	var at: int=band_ids.level.find(focus_id)
	var full_height:=maxf(48,54+font_delta*2);var gap:=24.0
	var cols:=clampi(int(rw/maxf(150,(180+font_delta*5)*context_density)),2,5)
	var cellw: float=(rw-gap*(cols-1))/cols
	var cellh:=maxf(34,36*context_density+font_delta*2)
	for direction in [-1,1]:
		var near: int=at+direction
		if near<0 or near>=band_ids.level.size():continue
		var y: float=reader_center.y+direction*(rh*.5+gap+full_height*.5)
		var id: String=band_ids.level[near];full_peers.append(id)
		_focus_card(id,Vector2(reader_x,y),Vector2(rw,full_height),true)
		var room: float=(y-full_height*.5-138) if direction<0 else (h-52-y-full_height*.5)
		var rows:=clampi(int(room/(cellh+gap)),0,3)
		for distance in range(2,2+cols*rows):
			var index: int=at+direction*distance
			if index<0 or index>=band_ids.level.size():break
			var slot:=distance-2;var col:=slot%cols if direction>0 else cols-1-slot%cols
			var row:=int(slot/cols)
			var x: float=reader_x-rw*.5+cellw*.5+col*(cellw+gap)
			var cy: float=y+direction*(full_height*.5+gap+cellh*.5+row*(cellh+gap))
			_focus_card(str(band_ids.level[index]),Vector2(x,cy),Vector2(cellw,cellh))
	packed_points=goals.duplicate();packed_bands=band_ids.duplicate(true)
	for label in tags.values():label.hide()
	for role in ["parents","children"]:
		for id in band_ids[role]:_tag(str(id),"PARENT" if id in roles.parents else "CHILD" if id in roles.children else "RELATED BRANCH")
	for button in role_references.values():button.hide()
	for role in pagers:
		for button in pagers[role]:button.hide()
	for label in counters.values():label.hide()
	headings.next.hide();move_hint.hide();resize_hint.hide()
	return fresh

func set_instruction_open(value:bool)->void:
	if instruction_open==value:return
	_settle();instruction_open=value
	if active:present(focus_id,false)

func _reader_rect() -> Rect2:
	var rect:=Rect2()
	for i in 4:
		var point: Vector2=forest.camera.unproject_position(to_global(Vector3((-.5 if i%2==0 else .5)*dimensions.x,(-.5 if i<2 else .5)*dimensions.y,.24)))
		rect=Rect2(point,Vector2.ZERO) if i==0 else rect.expand(point)
	return rect

func update_annotations() -> void:
	if not active:return
	var box:=_reader_rect()
	if instruction_button!=null:
		instruction_button.position=Vector2(box.end.x-38,box.position.y+10)
		close_button.position=instruction_button.position+Vector2(0,36)
	move_hint.hide();resize_hint.hide()
	for role in ["parents","level","children"]:
		var heading: Label=headings.previous if role=="level" else headings[role]
		var p: Vector3=layout_origin+Vector3((band_x[role]-band_x.level)/PIXELS_PER_UNIT,0,0)
		heading.position=Vector2(forest.camera.unproject_position(p).x-90,110)
		heading.visible=not band_ids[role].is_empty() and heading.position.x>10 and heading.position.x<forest.size.x-100
	for id in context_ids:
		var rect: Rect2=forest._card_screen_rect(str(id))
		var safe:=Rect2(12 if compact_wheel else float(app.wheel_width)+50,142,forest.size.x-(24 if compact_wheel else float(app.wheel_width)+65),forest.size.y-192)
		var visible: bool=id!=focus_id and focus_visible.has(id) and safe.encloses(rect) and not rect.intersects(box.grow(3))
		forest.plaques_3d[id].visible=visible
		if tags.has(id):
			tags[id].position=Vector2(rect.position.x+2,rect.end.y+2)
			tags[id].visible=visible and forest.reading_font_pixels(str(id))>=18
	for role in ["parents","level","children"]:
		var heading: Label=headings.previous if role=="level" else headings[role]
		var count: int=band_ids[role].filter(func(id):return forest.plaques_3d[id].visible).size()
		heading.text=("PARENT LEVEL" if role=="parents" else "SAME LEVEL" if role=="level" else "CHILD LEVEL")+" · %d / %d"%[count,band_ids[role].size()]
	annotations.show();attachment.hide()

func _draw_attachment() -> void:
	if not active:return
	var a: Vector3=forest.points[focus_id]+Vector3(forest.bounds[focus_id].x*.5,0,.23)
	var b: Vector3=position+Vector3(-dimensions.x*.5,clampf(a.y-position.y,-dimensions.y*.5+.3,dimensions.y*.5-.3),.23)
	var normal: Vector3=(b-a).cross(forest.camera.global_basis.z).normalized()*forest._pixel_world(a)*1.5
	var arrays: Array=[];arrays.resize(Mesh.ARRAY_MAX);arrays[Mesh.ARRAY_VERTEX]=PackedVector3Array([a-normal,a+normal,b+normal,a-normal,b+normal,b-normal])
	var mesh:=ArrayMesh.new();mesh.add_surface_from_arrays(Mesh.PRIMITIVE_TRIANGLES,arrays);attachment.mesh=mesh;attachment.show()

func reader_port(relation: Dictionary,outgoing: bool=true) -> Vector3:
	var slots: Array=forest.out_slots.get(focus_id,[]) if outgoing else forest.in_slots.get(focus_id,[]);var at: int=slots.find(forest._slot_key(relation))
	var y: float=((float(at)+1)/float(slots.size()+1)-.5)*dimensions.y*.65
	return to_global(Vector3(dimensions.x*(.5 if outgoing else -.5),y,.23))

func avoid_reader(start: Vector3,end: Vector3) -> PackedVector3Array:
	var rect:=Rect2(Vector2(position.x-dimensions.x*.5,position.y-dimensions.y*.5),dimensions).grow(.20)
	if maxf(start.x,end.x)<rect.position.x or minf(start.x,end.x)>rect.end.x:return PackedVector3Array()
	if start.x>=rect.end.x-.3 or end.x>=rect.end.x-.3 and start.x>rect.end.x:return PackedVector3Array()
	if not Geometry2D.segment_intersects_segment(Vector2(start.x,start.y),Vector2(end.x,end.y),Vector2(rect.position.x,rect.position.y),Vector2(rect.position.x,rect.end.y)) and not rect.has_point(Vector2((start.x+end.x)*.5,(start.y+end.y)*.5)):return PackedVector3Array()
	var upper: bool=(start.y+end.y)*.5>position.y
	var y: float=rect.end.y+.22 if upper else rect.position.y-.22
	var side:=1.0 if end.x>start.x else -1.0
	var curve:=Curve3D.new();curve.bake_interval=.12
	curve.add_point(start,Vector3.ZERO,Vector3(side*.4,0,0))
	curve.add_point(Vector3(start.x+side*.5,y,start.z),Vector3(0,-.4 if upper else .4,0),Vector3(side*.4,0,0))
	curve.add_point(Vector3(end.x-side*.5,y,end.z),Vector3(-side*.4,0,0),Vector3(0,-.4 if upper else .4,0))
	curve.add_point(end,Vector3(-side*.4,0,0),Vector3.ZERO)
	return curve.get_baked_points()

func _apply_context_positions() -> void:
	forest.visible_nodes.clear()
	for id in forest.plaques_3d:
		var card=forest.plaques_3d[id];card.position=forest.points[id];card.visible=id in context_ids
		if id in context_ids:
			forest.visible_nodes[id]=true
			card.set_context_strength(0 if id==focus_id or id in roles.parents or id in roles.children else .22)
			card.skin.position=Vector3.ZERO;card.skin.rotation=Vector3.ZERO
			card.title_label.show();card.glyph_label.hide()
	forest.star_atlas.world_points=base_atlas_points;forest._edge_dirty=true;forest._label_dirty=true

func _refresh_camera() -> void:
	forest.source_index=app.active_index;forest.depth_value=app.wheel_value
	forest._update_camera();forest._place_wheel();app._sync_source_controls();update_annotations()

func present(id: String,animate: bool=true) -> void:
	if not forest.points.has(id):return
	if active and id==focus_id and animating():return
	var opening:=not active;var changed:=id!=focus_id
	var departing_id:=focus_id
	var old_points: Dictionary=forest.points.duplicate();var old_context:=context_ids.duplicate()
	var old_position:=position;var old_scale:=scale
	var old_extents: Dictionary={}
	for key in context_ids:
		if forest.plaques_3d.has(key):old_extents[key]=forest.bounds[key]*Vector2(forest.plaques_3d[key].scale.x,forest.plaques_3d[key].scale.y)
	var rapid: bool=not opening and Time.get_ticks_msec()-last_navigation_ms<180
	if changed or opening:last_navigation_ms=Time.get_ticks_msec()
	if effect and effect.is_valid():effect.kill()
	if opening:
		base_points=forest.points.duplicate();base_bounds=forest.bounds.duplicate();base_edges=forest.edge_paths.duplicate(true);base_cache=forest._route_cache.duplicate(true);base_atlas_points=forest.star_atlas.world_points;saved_direction=forest.view_direction;layout_key=""

	active=true;forest.detail_active=true;focus_id=id;forest.overview_mode=false;forest.relation_mode=false;forest.relation_overlay.hide();tooltip.hide()
	var fresh:=_build_context()
	var node_goal: Vector3=goals[id]
	var reader_goal: Vector3=node_goal+Vector3((reader_x-band_x.level)/PIXELS_PER_UNIT,0,.12)+manual_offset
	var start_position: Vector3=old_points[id] if opening or (changed and not rapid) else old_position
	var incoming: Vector2=old_extents.get(id,base_bounds[id])
	var start_scale: Vector3=Vector3(incoming.x/dimensions.x,incoming.y/dimensions.y,1) if opening or (changed and not rapid) else old_scale
	var start_target: Vector3=forest.target;var start_size: float=forest.view_size;var start_direction: Vector3=forest.view_direction
	var goal_size: float=forest.size.y/PIXELS_PER_UNIT/detail_zoom
	var goal_target: Vector3=layout_origin
	reader_goal=goals[id]+Vector3(0,0,.12)
	var target_extents: Dictionary=forest.bounds.duplicate()
	var starts: Dictionary={}
	for key in goals:starts[key]=old_points[key] if key in old_context else goals[key]
	position=start_position;scale=start_scale;show();shell.accent=forest.plaques_3d[id].accent;shell.set_selected(true,not forest.reduced_motion)
	var handoff: bool=changed and not opening and animate and not forest.reduced_motion
	departing_surface.hide()
	if handoff and not rapid and departing_id in forest.plaques_3d:
		departing_surface.reparent(forest.plaques_3d[departing_id],false)
		departing_surface.mesh.size=forest.bounds[departing_id]-Vector2(.15,.15)
		departing_surface.position=Vector3(0,0,.28);departing_surface.show()
	var content_state: Dictionary={"loaded":not handoff or rapid}
	surface.visible=content_state.loaded
	if content_state.loaded:_content(forest.record_for(id))
	var goal_scale:=Vector3.ONE/detail_zoom
	if instruction_open:
		# The detail face sits in front of the context plane. Compensate that
		# perspective magnification so its projected rectangle matches the
		# same logical-pixel composition used by the adjacent native editor.
		var distance:float=goal_size/(2.0*tan(deg_to_rad(forest.camera.fov)*.5))
		var face_scale:float=(distance-(reader_goal.z-goal_target.z))/(distance+surface.position.z)
		goal_scale*=face_scale
		reader_goal.x=goal_target.x+(reader_goal.x-goal_target.x)*face_scale
		reader_goal.y=goal_target.y+(reader_goal.y-goal_target.y)*face_scale
	# Target geometry is prepared once. Godot transforms stretch the existing
	# shell during the morph; keep glyph size independent of that transform.
	for key in goals:forest.plaques_3d[key].resize_geometry(target_extents[key])
	# Cache final world-space geometry, never an animation frame or projection.
	var digest:=HashingContext.new();digest.start(HashingContext.HASH_SHA256)
	var route_bounds: Dictionary={}
	for key in goals:route_bounds[key]=target_extents[key]
	digest.update(var_to_bytes([focus_id,context_ids,goals,route_bounds,reader_goal,dimensions,goal_scale]))
	var route_key:=digest.finish().hex_encode()
	var final_paths: Dictionary=forest.detail_route_layouts.get(route_key,{})
	if not forest.detail_route_layouts.has(route_key):
		var saved_points: Dictionary=forest.points.duplicate()
		var saved_position:=position;var saved_scale:=scale
		position=reader_goal;scale=goal_scale
		for key in goals:
			forest.points[key]=goals[key];forest.plaques_3d[key].position=goals[key]
			forest.plaques_3d[key].skin.scale=Vector3.ONE
		var route_ids: Array=focus_visible.keys()
		if focus_id not in route_ids:route_ids.append(focus_id)
		final_paths=forest.prepare_planar_routes(route_ids,forest.points,target_extents,forest.full_relations)

		forest.detail_route_layouts[route_key]=final_paths
		forest.points=saved_points;position=saved_position;scale=saved_scale
	# Sample the departing path with native Curve3D once. Tween frames only
	# interpolate these prepared vertices; no solver or Curve bake runs there.
	var motion_paths: Array=[]
	var departing: Dictionary={}
	for edge in forest.edge_paths:departing[forest._slot_key(edge.relations[0])]=edge.points
	forest.edge_paths=base_edges.duplicate(true)
	for edge in forest.edge_paths:
		var key: String=forest._slot_key(edge.relations[0])
		if not final_paths.has(key):continue
		var target_path: PackedVector3Array=final_paths[key]
		var curve:=Curve3D.new()
		for point in departing.get(key,edge.points):curve.add_point(point)
		var start_path:=PackedVector3Array()
		var length:=curve.get_baked_length()
		for i in target_path.size():start_path.append(curve.sample_baked(length*float(i)/maxf(1,target_path.size()-1)))
		motion_paths.append({"edge":edge,"start":start_path,"target":target_path})
	var initial_alpha: float=app.reader.modulate.a
	var frame:=func(t: float):
		position=start_position.lerp(reader_goal,t);scale=start_scale.lerp(goal_scale,t)
		if handoff:
			scale-=goal_scale*sin(t*PI)*(.018 if rapid else .035)
			if not content_state.loaded and t>=50.0/180.0:
				departing_surface.hide();_content(forest.record_for(id));content_state.loaded=true;surface.show()
			app.reader.modulate.a=lerpf(initial_alpha,1,t) if rapid else (1-t/(50.0/180.0) if t<50.0/180.0 else (t-50.0/180.0)/(130.0/180.0))
		else:app.reader.modulate.a=1
		viewport.render_target_update_mode=SubViewport.UPDATE_ONCE
		for key in goals:
			forest.points[key]=starts[key].lerp(goals[key],t)
			var target_extent: Vector2=target_extents[key]
			var from_extent: Vector2=old_extents.get(key,target_extent)
			var extent:=from_extent.lerp(target_extent,t)
			var card=forest.plaques_3d[key]
			forest.bounds[key]=extent;card.scale=Vector3.ONE
			card.skin.scale=Vector3(extent.x/target_extent.x,extent.y/target_extent.y,1)
			card.title_label.scale=Vector3(1.0/card.skin.scale.x,1.0/card.skin.scale.y,1)
			if not from_extent.is_equal_approx(target_extent):
				card.title_label.modulate.a=absf(2*t-1)
			else:card.title_label.modulate.a=1
			# Row-wrap text fades while its physical plaque travels across the grid.
			# Keep the same object and animate its geometry; avoid overlapping glyphs.
			if absf(starts[key].x-goals[key].x)>target_extent.x*1.5 and absf(starts[key].y-goals[key].y)>.3:
				card.title_label.modulate.a=clampf(1-t/.08,0,1) if t<.5 else clampf((t-.7)/.3,0,1)
			if departing_surface.visible and departing_surface.get_parent()==card:
				departing_surface.mesh.size=extent-Vector2(.15,.15);card.title_label.modulate.a=0

		for motion in motion_paths:
			var path:=PackedVector3Array()
			for i in motion.target.size():path.append(motion.start[i].lerp(motion.target[i],t))
			motion.edge.points=path
		_apply_context_positions()
		forest.target=start_target.lerp(goal_target,t);forest.view_size=lerpf(start_size,goal_size,t);forest.view_direction=start_direction.lerp(Vector3(0,0,1),t).normalized();forest.depth_clearance=0
		_refresh_camera()
	if forest.reduced_motion or not animate:frame.call(1.0)
	else:
		# Tween runs after Node processing. Install the initial detail visibility
		# now, so the next render does not rebuild/draw the entire restored forest.
		frame.call(0.0)
		effect=create_tween().set_trans(Tween.TRANS_LINEAR);effect.tween_method(frame,0.0,1.0,.09 if rapid else .18);effect.finished.connect(app._trace_state)

func _settle() -> void:
	if effect and effect.is_valid() and effect.is_running():effect.custom_step(10)

func _plane_point(point: Vector2) -> Vector3:
	var hit=Plane(Vector3(0,0,1),forest.points[focus_id].z).intersects_ray(forest.camera.project_ray_origin(point),forest.camera.project_ray_normal(point))
	return hit if hit is Vector3 else forest.points[focus_id]

func zoom_at(_point: Vector2,factor: float) -> void:
	_settle();context_density=clampf(context_density*factor,.75,1.5);present(focus_id)

func pan_pixels(_delta: Vector2) -> void:
	pass # Detail layout is anchored; dragging never moves the scene.

func change_font(delta: int,reset: bool=false) -> void:
	_settle();font_delta=0 if reset else clampi(font_delta+delta,-6,12)
	app.reader_text.add_theme_font_size_override("normal_font_size",int((24+font_delta)*1.5))
	app.reader_text.add_theme_font_size_override("bold_font_size",int((24+font_delta)*1.5))
	present(focus_id,false)

func _make_reader_room() -> void:
	# Preserve reading columns when a user enlarges/moves the leaf. This is
	# local presentation geometry, never a change to the graph or browse cache.
	var left_limit: float=-INF
	for id in band_ids.level:left_limit=maxf(left_limit,forest.points[id].x+forest.bounds[id].x*.5+.35)
	if position.x-dimensions.x*.5<left_limit:
		var correction: float=left_limit-(position.x-dimensions.x*.5)
		position.x+=correction;manual_offset.x+=correction
	var shift:=0.0
	for id in band_ids.children:
		shift=maxf(shift,position.x+dimensions.x*.5+.5-(forest.points[id].x-forest.bounds[id].x*.5))
	if shift>0:
		for id in packed_bands.children:
			packed_points[id].x+=shift
			if goals.has(id):goals[id].x+=shift;forest.points[id].x+=shift
		band_x.children+=shift*PIXELS_PER_UNIT

func scroll_group(_role: String,direction: int) -> void:
	offsets[_role]=clampi(int(offsets[_role])+direction,0,maxi(0,band_ids[_role].size()-1));present(focus_id)

func close() -> void:
	if not active:return
	if effect and effect.is_valid():effect.kill()
	departing_surface.hide();surface.show()
	instruction_open=false;instruction_editor_rect=Rect2()
	active=false;forest.detail_active=false;annotations.hide();attachment.hide();drag_kind="";forest.view_direction=saved_direction;compact_wheel=false
	viewport.render_target_update_mode=SubViewport.UPDATE_DISABLED;app.reader.hide()
	var id:=focus_id;var start_scale:=scale;var start_position:=position
	var start_target: Vector3=forest.target;var start_size: float=forest.view_size
	_restore_geometry();forest.set_selection(app.displayed_selection())
	app._place_tree();app.tree_pan+=forest.ensure_reading_visible([app.displayed_selection()]);app._place_tree()
	var goal_target: Vector3=forest.target;var goal_size: float=forest.view_size
	var goal_scale:=Vector3(base_bounds[id].x/dimensions.x,base_bounds[id].y/dimensions.y,1)
	if forest.reduced_motion:hide();return
	forest.target=start_target;forest.view_size=start_size;forest._update_camera()
	effect=create_tween().set_trans(Tween.TRANS_CUBIC).set_ease(Tween.EASE_IN_OUT)
	effect.tween_method(func(t: float):
		position=start_position.lerp(base_points[id],t);scale=start_scale.lerp(goal_scale,t);forest.target=start_target.lerp(goal_target,t);forest.view_size=lerpf(start_size,goal_size,t)
		forest._update_camera();forest._place_wheel();app._sync_source_controls(),0.0,1.0,.28)
	effect.finished.connect(hide)

func handle_pointer(event: InputEvent) -> bool:
	if not active:return false
	if event is InputEventKey and event.pressed and event.ctrl_pressed:
		if event.keycode in [KEY_EQUAL,KEY_PLUS,KEY_KP_ADD]:change_font(2);return true
		if event.keycode in [KEY_MINUS,KEY_KP_SUBTRACT]:change_font(-2);return true
		if event.keycode==KEY_0:change_font(0,true);return true
	if event is InputEventMagnifyGesture:zoom_at(event.position,event.factor);return true
	if not (event is InputEventMouse or event is InputEventPanGesture):return false
	var point: Vector2=event.position
	# Native GUI action hits must reach Button before reader forwarding/picking.
	for button in [instruction_button,close_button]:
		if button.visible and button.get_rect().has_point(point):return false
	var box:=_reader_rect()
	if event is InputEventMouseButton:
		if event.button_index==MOUSE_BUTTON_RIGHT:return true
		if event.button_index in [MOUSE_BUTTON_WHEEL_UP,MOUSE_BUTTON_WHEEL_DOWN] and (event.button_mask&MOUSE_BUTTON_MASK_RIGHT)!=0:
			if event.pressed:zoom_at(point,pow(1.12,1 if event.button_index==MOUSE_BUTTON_WHEEL_UP else -1))
			return true
	if event is InputEventMouseMotion:
		tooltip.hide()
		var hovered: String=forest.pick_node(point)
		if not hovered.is_empty() and hovered in context_ids and not box.has_point(point):
			var names: Array=[]
			for parent in app.model.parents.get(hovered,[]):names.append(str(forest.record_for(parent).title))
			tooltip.text=str(forest.record_for(hovered).title)+("\nParents: "+" · ".join(names) if not names.is_empty() else "")
			tooltip.size=Vector2(minf(440,forest.size.x-40),0)
			tooltip.position=Vector2(clampf(point.x+16,20,forest.size.x-tooltip.size.x-20),clampf(point.y+24,135,forest.size.y-tooltip.size.y-50));tooltip.show()
	var hit=Plane(global_basis.z,global_position+global_basis.z*.24).intersects_ray(forest.camera.project_ray_origin(point),forest.camera.project_ray_normal(point))
	if hit!=null:
		var local: Vector3=to_local(hit)
		if absf(local.x)<=face_size.x*.5 and absf(local.y)<=face_size.y*.5:
			if event is InputEventPanGesture:app.reader_text.get_v_scroll_bar().value+=event.delta.y*24
			else:
				var forwarded=event.duplicate();forwarded.position=Vector2((local.x/face_size.x+.5)*viewport.size.x,(.5-local.y/face_size.y)*viewport.size.y)
				if forwarded is InputEventMouseMotion:forwarded.relative=Vector2.ZERO
				viewport.push_input(forwarded,true)
			viewport.render_target_update_mode=SubViewport.UPDATE_ONCE;return true
	if event is InputEventMouseMotion:return true
	if event is InputEventPanGesture:
		if absf(event.delta.y)>0.2:
			if point.x<box.position.x:scroll_group("parents",1 if event.delta.y>0 else -1)
			elif point.x>box.end.x:scroll_group("children",1 if event.delta.y>0 else -1)
		return true
	if event is InputEventMouseButton and event.button_index in [MOUSE_BUTTON_WHEEL_UP,MOUSE_BUTTON_WHEEL_DOWN]:
		if event.pressed:
			var direction: int=-1 if event.button_index==MOUSE_BUTTON_WHEEL_UP else 1
			if point.x<box.position.x:scroll_group("parents",direction)
			elif point.x>box.end.x:scroll_group("children",direction)
		return true
	return false

func animating() -> bool:
	return effect!=null and effect.is_valid() and effect.is_running()
