extends Node
## Host adapter only: Rust owns selection/validation. Reuse the process file
## hub, native readiness and the existing global presentation ConfigFile.
signal changed
static var _instance
const SETTINGS="user://idea-tree-presentation.cfg"
var backend
var watcher
var deadline

static func shared(tree:SceneTree):
	if not is_instance_valid(_instance):
		_instance=load("res://browsing_configuration.gd").new()
		_instance.name="IdeaBrowsingConfiguration"
		_instance.initialize()
		tree.root.add_child.call_deferred(_instance)
	return _instance

func initialize()->void:
	backend=ClassDB.instantiate("PolarBrowsingConfiguration")
	var path:="res://polar-browsing.itree.json"
	if not FileAccess.file_exists(path):path="res://../../itrees/polar-browsing.itree.json"
	var result=JSON.parse_string(backend.initialize(FileAccess.get_file_as_string(path)))
	if not bool(result.get("ok",false)):push_error(str(result));return
	deadline=preload("res://addons/polar_graphics/deadline.gd").new(_reload,.12)
	watcher=ClassDB.instantiate("PolarFileEvents");add_child(watcher)
	watcher.files_changed.connect(func():deadline.start())
	watcher.watch_failed.connect(_watch_failed)
	var saved:=ConfigFile.new();saved.load(SETTINGS)
	var requested:String=str(saved.get_value("browsing","path",""))
	if not requested.is_empty():backend.restore_file(requested,_read(requested))
	_watch()

func current()->Dictionary:return JSON.parse_string(backend.current())
func content()->String:return str(backend.content())
func select_file(path:String)->bool:
	var ok:bool=backend.select_file(path,_read(path))
	if ok:_save();_watch()
	changed.emit();return ok
func use_default()->void:
	deadline.stop();backend.use_default();_save();_watch();changed.emit()
func _reload()->void:
	var requested:String=str(current().requested_path)
	if requested.is_empty():return
	backend.reload(_read(requested));changed.emit()
func _read(path:String)->String:
	# Missing/partial files are handled by the same Rust last-valid contract.
	var file:=FileAccess.open(path,FileAccess.READ)
	return file.get_as_text() if file!=null else ""
func _watch()->void:
	var path:String=str(current().requested_path)
	var error:String=str(watcher.watch_files(PackedStringArray() if path.is_empty() else PackedStringArray([path])))
	if not error.is_empty():backend.watch_failed(error)
func _watch_failed(message:String)->void:backend.watch_failed(message);changed.emit()
func _save()->void:
	var saved:=ConfigFile.new();saved.load(SETTINGS)
	saved.set_value("browsing","path",str(current().requested_path))
	var error:=saved.save(SETTINGS)
	if error!=OK:backend.watch_failed("Cannot save configuration selection (%d)."%error)
func _exit_tree()->void:
	if deadline!=null:deadline.stop()
	_instance=null
