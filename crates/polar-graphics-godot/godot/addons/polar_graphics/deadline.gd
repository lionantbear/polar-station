extends RefCounted
## A one-shot host-clock deadline, independent of rendering and animation.
static var _tree_clock:WeakRef
var _events
var _timer:Timer
var _token:int
var _active:=false
var wait_time:=0.12

static func use_scene_tree_clock(tree:SceneTree)->void:
	# The standalone engine owns its main loop. Embedded Station never calls
	# this: its native wake fd/deadline adapter remains the clock owner.
	_tree_clock=weakref(tree)

func _init(callback:Callable,seconds:float=0.12)->void:
	wait_time=seconds
	var owner:=weakref(self)
	var tree=_tree_clock.get_ref() if _tree_clock!=null else null
	if tree!=null:
		_timer=Timer.new();_timer.one_shot=true;_timer.ignore_time_scale=true
		_timer.process_mode=Node.PROCESS_MODE_ALWAYS
		_timer.timeout.connect(func():
			var value=owner.get_ref()
			if value==null or not value._active:return
			value._active=false
			if callback.is_valid():callback.call())
		_timer.tree_entered.connect(func():
			var value=owner.get_ref()
			if value!=null and value._active:value._timer.start())
		tree.root.add_child.call_deferred(_timer)
		return
	_events=ClassDB.instantiate("PolarGraphicsEvents")
	_token=_events.subscribe(func():
		var value=owner.get_ref()
		if value==null:return
		value._active=false
		if callback.is_valid():callback.call())

func start(seconds:float=-1)->void:
	_active=true
	if _timer!=null:
		_timer.wait_time=maxf(.001,wait_time if seconds<0 else seconds)
		if _timer.is_inside_tree():_timer.start()
		return
	_events.schedule(_token,ceili(1000.0*(wait_time if seconds<0 else seconds)))

func stop()->void:
	_active=false
	if _timer!=null:_timer.stop()
	else:_events.cancel(_token)

func is_stopped()->bool:return not _active

func _notification(what:int)->void:
	if what!=NOTIFICATION_PREDELETE:return
	if _events!=null:_events.unsubscribe(_token)
	if is_instance_valid(_timer):_timer.stop();_timer.queue_free()
