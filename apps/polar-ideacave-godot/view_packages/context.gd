extends RefCounted
## SDK read/intent port. Snapshot copies cannot mutate workspace records/history.
signal snapshot_changed
signal selection_changed
signal reference_requested(reference:Dictionary)
var _model
var _navigation
func _init(model,navigation)->void:
	_model=model;_navigation=navigation
	_navigation.changed.connect(_changed)
func _changed(_reason:String)->void:selection_changed.emit()
func snapshot()->Dictionary:
	return {"records":_model.records.duplicate(true),"ordered_ids":_model.ordered_ids.duplicate(),"relations":_model.relation_edges.duplicate(true)}
func selection()->Dictionary:
	return {"id":_navigation.selected_id,"occurrence":_navigation.selected_occurrence}
func select_node(id:String)->bool:
	return str(_navigation.select_node(id).get("error","")).is_empty()
func navigate(direction:String,peers:Array=[],parents:Array=[])->bool:
	return _navigation.navigate_visible(direction,peers,parents)
func travel(ahead:bool)->void:_navigation.travel(ahead)

func request_reference(reference:Dictionary)->void:reference_requested.emit(reference.duplicate(true))

func replace_snapshot(model,navigation)->void:
	if _navigation.changed.is_connected(_changed):_navigation.changed.disconnect(_changed)
	_model=model;_navigation=navigation;_navigation.changed.connect(_changed)
	snapshot_changed.emit();selection_changed.emit()
