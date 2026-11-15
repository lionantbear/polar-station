extends SceneTree
## Standalone authoring tool: no app build, registration or native runtime needed.
func _initialize()->void:
	var args:=OS.get_cmdline_user_args()
	if args.size()!=2:
		push_error("Usage: godot --headless --script export.gd -- PACKAGE_DIRECTORY OUTPUT.polar-view.zip");quit(2);return
	var source:=args[0].simplify_path()
	var manifest=JSON.parse_string(FileAccess.get_file_as_string(source.path_join("manifest.json")))
	if not manifest is Dictionary or manifest.get("schema")!="polar.graphics.view-package.v1":
		push_error("Invalid package manifest");quit(2);return
	var zip:=ZIPPacker.new()
	if zip.open(args[1])!=OK:push_error("Cannot create export");quit(2);return
	var files:=PackedStringArray(["manifest.json"])
	_collect(source,"view_packages/"+str(manifest.id)+"/"+str(manifest.version),files)
	for path in files:
		if zip.start_file(path)!=OK or zip.write_file(FileAccess.get_file_as_bytes(source.path_join(path)))!=OK or zip.close_file()!=OK:
			zip.close();push_error("Export failed: "+path);quit(2);return
	if zip.close()!=OK:push_error("Cannot finalize export");quit(2);return
	print(args[1]);quit()

func _collect(root:String,relative:String,result:PackedStringArray)->void:
	for file in DirAccess.get_files_at(root.path_join(relative)):
		if not file.ends_with(".uid"):result.append(relative.path_join(file))
	for directory in DirAccess.get_directories_at(root.path_join(relative)):
		_collect(root,relative.path_join(directory),result)
