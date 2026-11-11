extends RefCounted
## Read-only adapter: original records/relations remain shared. Only browsing
## parents and group membership come from Rust; no file-per-layer classification.
const MAX_COMPOSITION_DEPTH:=64
var original
var records:Dictionary
var ordered_ids:Array[String]=[]
var source_roots:Array[String]=[]
var children:Dictionary={}
var parents:Dictionary={}
var neighbors:Dictionary
var relation_edges:Array
var presentation_revision:=""
var organization_revision:=""
var root_key:=""
var initial_selection:=""
var projection_actual_nodes:=0
var _composition:Dictionary={}
var display_groups:Dictionary={}
var display_membership:Dictionary={}
var reference_relations:Array=[]
var references:Dictionary={}
var _reference_ids:Dictionary={}
func canonical(id:String)->String:return str(references[id].node) if references.has(id) else id
func group_reference_for(id:String)->Dictionary:return references.get(id,{})
func configure(source,projection:Dictionary)->void:
	original=source;records=source.records.duplicate();neighbors=source.neighbors;relation_edges=source.relation_edges
	ordered_ids.assign(source.ordered_ids)
	organization_revision=JSON.stringify(projection.parents).sha256_text()
	root_key=str(projection.root);initial_selection=root_key;projection_actual_nodes=source.projection_actual_nodes
	source_roots.append(root_key)
	presentation_revision=JSON.stringify(source.source_roots.map(func(id):return source.identity(id))).sha256_text()
	display_membership=projection.membership.duplicate()
	for group:Dictionary in projection.groups:display_groups[str(group.id)]=group.members.duplicate()
	for ref:Dictionary in projection.get("references",[]):
		var id:String=str(ref.id);references[id]=ref
		var record:Dictionary=source.records[str(ref.node)].duplicate()
		record["key"]=id;record["group_reference"]=ref
		record["canonical_id"]=str(ref.node)
		records[id]=record;display_groups[str(ref.group)].append(id);display_membership[id]=str(ref.group)
		if not _reference_ids.has(str(ref.group)):_reference_ids[str(ref.group)]={}
		_reference_ids[str(ref.group)][str(ref.node)]=id
	for id in records:children[id]=[];parents[id]=[];_composition[id]=[]
	var tree_edges:Dictionary={}
	for child:String in projection.parents:
		var parent:Dictionary=projection.parents[child]
		tree_edges[int(parent.edge)]=true
		var edge:Dictionary=relation_edges[int(parent.edge)].duplicate()
		edge["slot"]=str(parent.slot)
		var from:String=str(parent.from)
		var a:String=str(projection.membership[from]);var b:String=str(projection.membership[child])
		if a==b:_add_browse(from,child,edge)
		else:
			# Rust already chose the browsing parent, including reverse policies.
			# Only map its endpoints to occurrences in each local display group.
			_add_browse(from,str(_reference_ids[a][child]),edge)
			_add_browse(str(_reference_ids[b][from]),child,edge)
	for item:Dictionary in projection.get("reference_edges",[]):
		var edge:Dictionary=relation_edges[int(item.edge)].duplicate()
		edge["from"]=str(item.from);edge["to"]=str(item.to)
		edge["slot"]="@reference/"+str(item.group)+"/"+str(item.edge)
		edge["display_reference"]=true;reference_relations.append(edge)
		if not tree_edges.has(int(item.edge)):_add_browse(str(item.from),str(item.to),edge)

func _add_browse(from:String,to:String,original_edge:Dictionary)->void:
	var edge:Dictionary=original_edge.duplicate()
	edge["from"]=from;edge["to"]=to;edge["composition"]=true;edge["derived_browsing_edge"]=true
	_composition[from].append(edge)
	if not to in children[from]:children[from].append(to)
	if not from in parents[to]:parents[to].append(from)

func composition_edges(id:String)->Array:return _composition.get(id,[])
func identity(id:String)->String:return original.identity(canonical(id))
func resolve(value:String)->String:return original.resolve(value)
