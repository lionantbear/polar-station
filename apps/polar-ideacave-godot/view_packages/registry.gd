extends RefCounted
## Trusted local packages. Rust validates archives; Godot mounts namespaced resources.
signal changed
signal selection_changed
static var _selection
const SELECTION_PATH="user://idea-tree-presentation.cfg"
signal reference_requested(reference:Dictionary)
static var _observers:Array[WeakRef]=[]
static var _mounted:Dictionary={}
const METHODS=["browser_state","reader_rect","editor_rect","set_instruction_visible","execute_action","resize_view","debug_state","set_active","set_preferences","snapshot","restore_view","sync_selection","show_details","needs_continuous_frames"]
var browsers:Dictionary={}
var themes:Dictionary={}
var installed:Array=[]
var error:=""
var store
var root:String=ProjectSettings.globalize_path("user://view-packages")

func _init()->void:
    if _selection==null:
        _selection=ClassDB.instantiate("PolarViewSelection")
        var saved:=ConfigFile.new()
        saved.load(SELECTION_PATH)
        _selection.select(str(saved.get_value("presentation","browser","depth_forest")),str(saved.get_value("presentation","theme","default")))
    _observers=_observers.filter(func(item):return item.get_ref()!=null)
    _observers.append(weakref(self))
    browsers={"mind_map":{"title":"Mind Map","entry":"","modes":["two_d","layered"]},"depth_forest":{"title":"Depth Forest","entry":"res://station_perspective_browser.gd","modes":["layered"]}}
    themes={"default":{"title":"Default","entry":"res://addons/polar_graphics/themes/default.tres"},"christmas":{"title":"Christmas","entry":"res://addons/polar_graphics/themes/christmas.tres"}}
    if ClassDB.class_exists("PolarViewPackages"):store=ClassDB.instantiate("PolarViewPackages")

func refresh()->void:
    # Remove only external entries. Built-ins and external browsers share creation/dispatch.
    for collection in [browsers,themes]:
        for id in collection.keys():
            if str(id).contains("/"):collection.erase(id)
    installed=[]
    if store==null:return
    var result=JSON.parse_string(store.catalog(root))
    if not result.ok:error=result.error;return
    for package in result.value:
        var manifest:Dictionary=package.manifest
        var key:String=manifest.id+"/"+manifest.version
        if _mounted.has(key) and _mounted[key]!=package.digest:
            error="Different resources for an already mounted version; publish a new version or restart.";continue
        for kind in ["browsers","themes"]:
            var collection:Dictionary=browsers if kind=="browsers" else themes
            for entry in manifest[kind]:
                var descriptor:Dictionary=entry.duplicate(true)
                descriptor.package_key=key
                descriptor.entry="res://"+str(entry.entry)
                descriptor.title=str(entry.title)+" · "+str(manifest.version)
                collection[key+"/"+str(entry.id)]=descriptor
        installed.append(package)

func choices(kind:String)->Array:
    var result:Array=[]
    var collection:Dictionary=browsers if kind=="browsers" else themes
    for id in collection:
        # Compatibility actions already expose the built-ins in older hosts.
        if str(id).contains("/"):result.append({"id":id,"title":collection[id].title})
    return result

func package_choices()->Array:
    var result:Array=[]
    for package in installed:
        result.append({"id":package.manifest.id+"/"+package.manifest.version,"title":package.manifest.title+" · "+package.manifest.version})
    return result

func create_browser(id:String,model,navigation,state:Dictionary={}):
    if not browsers.has(id):error="Browser is not installed";return null
    if not activate_entry(browsers[id]):return null
    var script=load(browsers[id].entry)
    if not script is Script or not script.can_instantiate():error="Browser entry is not an instantiable script";return null
    var candidate=script.new()
    if not _valid_browser(candidate,id):
        if candidate is Node:candidate.free()
        return null
    if candidate.has_method("prepare_view_state"):candidate.prepare_view_state(state)
    if candidate.has_method("attach_context"):
        var context=preload("res://view_packages/context.gd").new(model,navigation)
        context.reference_requested.connect(_forward_reference)
        candidate.set_meta("polar_view_context",context)
        candidate.attach_context(context)
    else:candidate.attach(model,navigation)
    if candidate.has_signal("reference_requested"):
        candidate.reference_requested.connect(_forward_reference)
    return candidate

func _valid_browser(candidate,id:String)->bool:
    if not candidate is Control:error="Browser must extend Control";return false
    for method in METHODS:
        if not candidate.has_method(method):error="Browser interface is missing "+method;return false
    for event in ["state_changed","instruction_requested"]:
        if not candidate.has_signal(event):error="Browser interface is missing "+event;return false
    if candidate.has_method("attach_context"):return true
    if not id.contains("/") and candidate.has_method("attach"):return true
    error="External browser must use the SDK context port"
    return false

func command(action:String,query:String)->String:
    if store==null:return "View package service is unavailable in this build."
    var result:Variant
    if action=="install_package":result=JSON.parse_string(store.install(query,root))
    else:
        var parts:PackedStringArray=query.split("/")
        if action=="export_package":
            var request=JSON.parse_string(query)
            if not request is Dictionary:return "Invalid export request"
            parts=str(request.get("id","")).split("/")
            if parts.size()!=2:return "Invalid package identity"
            result=JSON.parse_string(store.export_package(root,parts[0],parts[1],str(request.get("destination",""))))
        else:
            if parts.size()!=2:return "Invalid package identity"
            result=JSON.parse_string(store.uninstall(root,parts[0],parts[1]))
    if not result.ok:return str(result.error)
    for observer in _observers:
        var registry=observer.get_ref()
        if registry!=null and registry.root==root:
            registry.refresh();registry.changed.emit()
    return "View package installed." if action=="install_package" else ("View package exported." if action=="export_package" else "View package uninstalled.")

func current_selection()->Dictionary:
    return JSON.parse_string(_selection.current())

func activate_entry(descriptor:Dictionary)->bool:
    var key:=str(descriptor.get("package_key",""))
    if key.is_empty() or _mounted.has(key):return true
    for package in installed:
        if package.manifest.id+"/"+package.manifest.version!=key:continue
        if not ProjectSettings.load_resource_pack(package.archive,false):
            error="Could not mount "+str(package.manifest.title);return false
        _mounted[key]=package.digest
        return true
    error="Package is not installed"
    return false

func select_presentation(browser:String,theme:String)->bool:
    if not browsers.has(browser) or not themes.has(theme):return false
    if not activate_entry(browsers[browser]) or not activate_entry(themes[theme]):return false
    if browser!="mind_map":
        var script=load(browsers[browser].entry)
        if not script is Script or not script.can_instantiate():return false
        var candidate=script.new()
        var valid:=_valid_browser(candidate,browser)
        if candidate is Node:candidate.free()
        if not valid:return false
    var visual=load(themes[theme].entry)
    if visual==null or not visual.has_method("valid") or not visual.valid():return false
    if not _selection.select(browser,theme):return false
    var saved:=ConfigFile.new()
    saved.load(SELECTION_PATH)
    saved.set_value("presentation","browser",browser);saved.set_value("presentation","theme",theme)
    var result:=saved.save(SELECTION_PATH)
    if result!=OK:push_warning("Cannot save global presentation: %d"%result)
    for observer in _observers:
        var registry=observer.get_ref()
        if registry!=null:registry.selection_changed.emit()
    return true

func update_browser(browser,model,navigation,state:Dictionary={})->void:
    if browser.has_method("prepare_view_state"):browser.prepare_view_state(state)
    if browser.has_meta("polar_view_context"):
        browser.get_meta("polar_view_context").replace_snapshot(model,navigation)
    elif browser.has_method("replace_context"):
        browser.replace_context(model,navigation)

func _forward_reference(reference:Dictionary)->void:reference_requested.emit(reference)

func unbind_browser(browser)->void:
    var emitter=browser.get_meta("polar_view_context") if browser.has_meta("polar_view_context") else browser
    if emitter.has_signal("reference_requested") and emitter.reference_requested.is_connected(_forward_reference):
        emitter.reference_requested.disconnect(_forward_reference)

func bind_browser(browser,model,navigation,state:Dictionary={})->void:
    var emitter=browser.get_meta("polar_view_context") if browser.has_meta("polar_view_context") else browser
    if emitter.has_signal("reference_requested") and not emitter.reference_requested.is_connected(_forward_reference):
        emitter.reference_requested.connect(_forward_reference)
    update_browser(browser,model,navigation,state)
