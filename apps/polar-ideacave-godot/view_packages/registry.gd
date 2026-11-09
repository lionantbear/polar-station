extends RefCounted
## Trusted local packages. Rust validates archives; Godot mounts namespaced resources.
signal changed
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
    _observers=_observers.filter(func(item):return item.get_ref()!=null)
    _observers.append(weakref(self))
    browsers={"mind_map":{"title":"Mind Map","entry":"","modes":["two_d","layered"]},"depth_forest":{"title":"Depth Forest","entry":"res://depth_forest/browser.gd","modes":["layered"]}}
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
        if not _mounted.has(key):
            if not ProjectSettings.load_resource_pack(package.archive,false):
                error="Could not mount "+manifest.title;continue
            _mounted[key]=package.digest
        for kind in ["browsers","themes"]:
            var collection:Dictionary=browsers if kind=="browsers" else themes
            for entry in manifest[kind]:
                var descriptor:Dictionary=entry.duplicate(true)
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

func create_browser(id:String,model,navigation):
    if not browsers.has(id):error="Browser is not installed";return null
    var script=load(browsers[id].entry)
    if not script is Script or not script.can_instantiate():error="Browser entry is not an instantiable script";return null
    var candidate=script.new()
    if not candidate is Control:error="Browser must extend Control";return null
    for method in METHODS:
        if not candidate.has_method(method):
            error="Browser interface is missing "+method;candidate.free();return null
    for event in ["state_changed","instruction_requested"]:
        if not candidate.has_signal(event):error="Browser interface is missing "+event;candidate.free();return null
    if candidate.has_method("attach_context"):
        var context=preload("res://view_packages/context.gd").new(model,navigation)
        context.reference_requested.connect(func(reference):reference_requested.emit(reference))
        candidate.attach_context(context)
    elif not id.contains("/") and candidate.has_method("attach"):
        candidate.attach(model,navigation)
    else:
        error="External browser must use the SDK context port";candidate.free();return null
    if candidate.has_signal("reference_requested"):
        candidate.reference_requested.connect(func(reference):reference_requested.emit(reference))
    return candidate

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
