extends Node
## Workspace-owned local-preview loading. Browser packages never perform I/O.
## Explicit roots define the automatic frontier; loading a child never grows it.
signal ready_model(candidate, paths:PackedStringArray)
signal changed
const Model=preload("res://itree_view_model.gd")
var roots:=PackedStringArray()
var paths:=PackedStringArray()
var entries:Array[Dictionary]=[]
var paused:=false
var _queue:Array[Dictionary]=[]
var _thread:Thread
var _generation:=0
var _active:Dictionary={}
var _model
var apply_candidate:Callable
var _staged_candidate
var publication_count := 0
var _published_model
var _published_paths:=PackedStringArray()

func configure(value, explicit:PackedStringArray, loaded:PackedStringArray)->void:
	_generation+=1;_queue.clear();entries.clear();_staged_candidate=null
	roots=explicit.duplicate();paths=loaded.duplicate();_model=value
	_published_model=value;_published_paths=loaded.duplicate()
	_discover();changed.emit();_pump()

func _locate(source:String,hint:String)->String:
	if hint.is_absolute_path():return hint.simplify_path()
	# Existing hints are either document-relative or repository-relative.
	var directory:=source.get_base_dir()
	while not directory.is_empty():
		var candidate:=directory.path_join(hint).simplify_path()
		if FileAccess.file_exists(candidate):return candidate
		if DirAccess.dir_exists_absolute(directory.path_join(".git")) or FileAccess.file_exists(directory.path_join(".git")):break
		var parent:=directory.get_base_dir()
		if parent==directory:break
		directory=parent
	return source.get_base_dir().path_join(hint).simplify_path()

func _discover()->void:
	var previous:Dictionary={}
	for entry in entries:previous[entry.path]=entry
	entries.clear()
	var pending:Dictionary={}
	for edge in _model.relation_edges:
		var owner:Dictionary=_model.records[edge.from]
		var hint:=str(edge.properties.get("document",""))
		if hint.is_empty() or edge.target_kind=="local":continue
		var path:=_locate(str(owner.source),hint)
		if path in paths:continue
		var automatic:bool=bool(edge.composition) and str(owner.source) in roots
		if not pending.has(path):
			var old:Dictionary=previous.get(path,{})
			pending[path]={"path":path,"label":path.get_file(),"status":old.get("status","pending" if automatic else "not_loaded"),"reason":old.get("reason",""),"automatic":automatic,"references":[]}
		pending[path].automatic=bool(pending[path].automatic) or automatic
		if automatic and pending[path].status=="not_loaded":pending[path].status="pending"
		pending[path].references.append(edge.reference.duplicate(true))
	for path in pending:
		var entry:Dictionary=pending[path]
		entries.append(entry)
		if entry.automatic and entry.status=="pending" and not _queued(path):_queue.append(entry)

func _queued(path:String)->bool:
	if not _active.is_empty() and _active.get("generation",-1)==_generation and _active.path==path:return true
	for entry in _queue:
		if entry.path==path:return true
	return false

func request(path:String)->void:
	for entry in entries:
		if entry.path!=path:continue
		else:
			if _queued(path):return
			entry.status="pending";entry.reason="";_queue.push_front(entry)
			paused=false;changed.emit();_pump();return


func request_reference(reference:Dictionary)->bool:
	var matches:Array=[]
	for entry in entries:
		if reference in entry.references:matches.append(entry.path)
	if matches.size()!=1:return false
	request(str(matches[0]));return true

func toggle_pause()->void:
	paused=not paused
	if paused and _thread==null:_publish_staged()
	changed.emit();_pump()

func _pump()->void:
	if paused or _thread!=null or _queue.is_empty():return
	_active=_queue.pop_front();_active.status="loading";_active.generation=_generation
	var job:=_active.duplicate(true);var generation:=_generation
	var source_paths:=paths.duplicate();source_paths.append(str(job.path))
	_thread=Thread.new()
	var error:=_thread.start(func():
		var candidate=Model.new()
		var message:=""
		if not candidate.load_files(source_paths):message=candidate.error
		if message.is_empty():message=_verify(candidate,job)
		call_deferred("_completed",generation,candidate,source_paths,message)
	)
	if error!=OK:
		_thread=null;_active.status="failed";_active.reason="Could not start file reader";_active={}
	changed.emit()

func _verify(candidate,job:Dictionary)->String:
	for reference in job.references:
		var matches:=0
		for id in candidate.ordered_ids:
			var record:Dictionary=candidate.records[id]
			if record.source!=job.path:continue
			if reference.get("kind")=="object" and record.uuid==reference.get("uuid"):matches+=1
			elif reference.get("kind")=="tree":
				if record.key!=reference.get("node") or record.source_sha256!=str(reference.get("content",{}).get("id","")).trim_prefix("sha256:"):continue
				if reference.has("uuid") and record.uuid!=reference.uuid:continue
				if reference.get("content",{}).has("size_bytes") and record.source_size_bytes!=reference.content.size_bytes:continue
				matches+=1
		if matches!=1:return "Referenced identity/version does not match the hinted file"
	return ""

func _completed(generation:int,candidate,loaded:PackedStringArray,message:String)->void:
	if _thread!=null:_thread.wait_to_finish();_thread=null
	var job:=_active;_active={}
	if generation==_generation:
		if message.is_empty():
			paths=loaded;_model=candidate;_staged_candidate=candidate
			_discover()
		else:job.status="failed";job.reason=message
		# Finish the existing finite frontier before rebuilding GPU scenes.
		# Pausing still publishes already-validated progress, never partial JSON.
		if _queue.is_empty() or paused:_publish_staged()
	changed.emit();_pump()

func _publish_staged()->void:
	if _staged_candidate==null:return
	var staged=_staged_candidate;_staged_candidate=null
	if not apply_candidate.is_valid() or apply_candidate.call(staged,paths):
		_published_model=staged;_published_paths=paths.duplicate()
		publication_count+=1;ready_model.emit(staged,paths)
	else:
		_model=_published_model;paths=_published_paths.duplicate();_queue.clear()
		paused=true
		for entry in entries:
			if entry.path not in paths:
				entry.status="failed";entry.reason="Related tree could not be applied; existing view retained"

func _exit_tree()->void:
	_generation+=1;_queue.clear()
	if _thread!=null:_thread.wait_to_finish();_thread=null
