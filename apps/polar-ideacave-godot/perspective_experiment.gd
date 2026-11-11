extends "res://perspective_runtime.gd"
## Standalone review shell; the embedded host uses the same runtime.
func _ready()->void:
	preload("res://addons/polar_graphics/deadline.gd").use_scene_tree_clock(get_tree())
	get_window().title="Polar Browsing — Perspective Experiment"
	get_window().content_scale_size=Vector2i.ZERO
	get_window().content_scale_mode=Window.CONTENT_SCALE_MODE_CANVAS_ITEMS
	get_window().content_scale_factor=maxf(1.0,DisplayServer.screen_get_scale())
	column=VBoxContainer.new();column.set_anchors_and_offsets_preset(Control.PRESET_FULL_RECT);add_child(column)
	status=Label.new();status.clip_text=true;status.text_overrun_behavior=TextServer.OVERRUN_TRIM_ELLIPSIS;status.add_theme_font_size_override("font_size",15);status.text="Loading existing Polar I-trees…";column.add_child(status);status.hide()
	await get_tree().process_frame
	var started:=Time.get_ticks_usec()
	var repo:=ProjectSettings.globalize_path("res://../..").simplify_path()
	var config_path:=repo.path_join("itrees/polar-browsing.itree.json")
	var config_text:=FileAccess.get_file_as_string(config_path)
	var config=JSON.parse_string(config_text)
	if not config is Dictionary:_fail("Cannot read Polar browsing configuration");return
	var paths:=PackedStringArray()
	for source:Dictionary in config.nodes["input-graph"].properties.sources:paths.append(repo.path_join(str(source.path)))
	if not original.load_files(paths):_fail(original.error);return
	if not configure_view(original,config_text):return
	timing["read_and_index_ms"]=(Time.get_ticks_usec()-started)/1000.0
	_build_footer();_build_search();_build_filter()
	if not switch_perspective(1):return
	await get_tree().process_frame
	_build_map()
	ready_for_review=true;_update_status()
	print("PERSPECTIVE_EXPERIMENT_READY ",JSON.stringify({"nodes":original.records.size(),"groups":projection.groups.size(),"level":projection.level,"timing":timing}))


func _build_footer()->void:
	var bar:=HBoxContainer.new();bar.add_theme_constant_override("separation",8);column.add_child(bar)
	var title:=Label.new();title.text="Idea";bar.add_child(title)
	input=LineEdit.new();input.placeholder_text="Describe a change…";input.size_flags_horizontal=Control.SIZE_EXPAND_FILL;bar.add_child(input)
	var find:=Button.new();find.text="Find";find.pressed.connect(open_search);bar.add_child(find)
	filter_button=Button.new();filter_button.text="More";filter_button.pressed.connect(func():filter_menu.position=Vector2i(filter_button.global_position+Vector2(0,filter_button.size.y));filter_menu.popup());bar.add_child(filter_button)
	var detail:=Button.new();detail.text="Details";detail.pressed.connect(func():browser.show_details());bar.add_child(detail)
	var send:=Button.new();send.text="Send";send.pressed.connect(func():status.text="Experimental host has no Codex session bound. Instruction draft retained.");bar.add_child(send)


func _build_search()->void:
	search_window=PopupPanel.new();search_window.title="Find · Loaded graph";add_child(search_window)
	var box:=VBoxContainer.new();search_window.add_child(box)
	var heading:=Label.new();heading.text="Find · Loaded graph · independent of Filter";box.add_child(heading)
	search_input=LineEdit.new();search_input.placeholder_text="Title, content or UUID";box.add_child(search_input)
	search_results=ItemList.new();search_results.custom_minimum_size=Vector2(620,340);box.add_child(search_results)
	search_input.text_changed.connect(_search)
	search_input.text_submitted.connect(func(_value):if not _results.is_empty():locate_result(0))
	search_results.item_activated.connect(locate_result)
	search_window.popup_hide.connect(func():if browser:browser.grab_focus())

func _build_filter()->void:
	filter_menu=PopupMenu.new();add_child(filter_menu)
	filter_menu.add_item("Filter: Clear",0);filter_menu.add_item("Emphasize development task type",1)
	filter_menu.add_separator();filter_menu.add_item("2D / 2.5D",2)
	filter_menu.id_pressed.connect(func(id:int):
		if id==2:
			browser.forest.reading_direction=Vector3(0,0,1) if browser.forest.reading_direction!=Vector3(0,0,1) else Vector3(.12,.08,1).normalized()
			browser._place_sources()
		else:apply_filter("filter-clear" if id==0 else "filter-development-tasks"))


func open_search()->void:
	search_window.popup_centered(Vector2i(660,420));search_input.grab_focus()

func _search(query:String)->void:
	var results=JSON.parse_string(backend.search(query))
	_results=results if results is Array else [];search_results.clear()
	for item:Dictionary in _results:
		var scope:="Current perspective" if projection.membership.has(str(item.id)) else "Outside current perspective"
		search_results.add_item("%s  ·  %s" % [str(item.title),scope])

func locate_result(index:int)->void:
	if index<0 or index>=_results.size():return
	locate_node(str(_results[index].id))
