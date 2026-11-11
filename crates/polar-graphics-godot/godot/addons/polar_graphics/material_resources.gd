extends RefCounted
## Immutable renderer variants held only by their actual scene consumers.
static func standard(properties:Dictionary,texture:Texture2D=null)->StandardMaterial3D:
	var signature:Array=[properties,texture.resource_path if texture!=null and not texture.resource_path.is_empty() else (texture.get_instance_id() if texture!=null else 0)]
	var key:="res://.polar-material/"+var_to_bytes(signature).hex_encode().sha256_text()
	var existing:=ResourceLoader.get_cached_ref(key) as StandardMaterial3D
	if existing!=null:return existing
	var material:=StandardMaterial3D.new()
	for name in properties:material.set(name,properties[name])
	if texture!=null:material.albedo_texture=texture
	material.take_over_path(key)
	return material

static func shader(program:Shader,parameters:Dictionary)->ShaderMaterial:
	# ResourceLoader's cache is weak: only the actual consumers own variants.
	# Sort uniforms so call order never becomes part of renderer identity.
	var names:=parameters.keys();names.sort()
	var signature:Array=[program.get_instance_id()]
	for name in names:signature.append([name,parameters[name]])
	var key:="res://.polar-material/shader/"+var_to_bytes(signature).hex_encode().sha256_text()
	var existing:=ResourceLoader.get_cached_ref(key) as ShaderMaterial
	if existing!=null:return existing
	var material:=ShaderMaterial.new();material.shader=program
	for name in names:material.set_shader_parameter(name,parameters[name])
	material.take_over_path(key)
	return material
