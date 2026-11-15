extends RefCounted

## Read-only display encoding of the existing preview records. This does not
## admit content, infer status, or choose an object's latest version.
static func sources(model) -> Dictionary:
	var result: Dictionary = {}
	for id in model.source_roots:
		var source := str(model.records[id].source)
		if not result.has(source): result[source] = result.size()
	return result

static func node(record: Dictionary, source_indices: Dictionary, root: bool = false) -> Dictionary:
	var kind := str(record.get("type", ""))
	var token := kind.trim_prefix("polar.").trim_prefix("idea.").get_slice(".",0).to_lower()
	# Exact aliases already used by the real source trees. Do not infer an
	# unknown type from arbitrary interior namespace words.
	var aliases:Dictionary={"polar.development.design.v1":"design","polar.development.task.v1":"task","polar.development.decision.v1":"decision","polar.tree.definition.rule.v1":"rule","polar.document.group.v1":"group"}
	token=str(aliases.get(kind,token))
	var shapes := {"request":"capsule", "decision":"diamond", "task":"rounded_block", "design":"hexagon", "rule":"double_plate"}
	var icons := {"capsule":"▰", "diamond":"◇", "rounded_block":"▢", "hexagon":"⬡", "double_plate":"▤", "generic":"○"}
	var shape := str(shapes.get(token, "generic"))
	var status = record.get("properties", {}).get("status", "")
	return {"source":str(record.get("source", "")), "source_index":int(source_indices.get(str(record.get("source", "")),0)),
		"type":kind, "shape":shape, "icon":str(icons[shape]), "status":status if status is String else JSON.stringify(status),
		"container":root or token in ["tree","collection","group"], "external":bool(record.get("external",false))}

static func relation(edge: Dictionary) -> Dictionary:
	var name := str(edge.get("type", ""))
	var token := name.trim_prefix("polar.").get_slice(".",0).replace("-","_").to_lower()
	var styles := {"contains":"solid", "fs":"solid", "implements":"solid", "uses":"long_dash", "context":"dotted", "defined_by":"double", "derived_from":"dash_dot"}
	return {"name":name, "line_style":str(styles.get(token,"solid")), "arrow":true,
		"identity":JSON.stringify([str(edge.get("from","")),str(edge.get("slot","")),edge.get("reference",{})]),
		"resolved":bool(edge.get("resolved",false))}
