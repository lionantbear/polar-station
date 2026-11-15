extends Node3D
## Native polished port resources, shared across all forest instances.
const Materials=preload("res://addons/polar_graphics/material_resources.gd")
static var _ring:TorusMesh
static var _core:SphereMesh
func configure(accent:Color)->void:
	if _ring==null:
		_ring=TorusMesh.new();_ring.inner_radius=.095;_ring.outer_radius=.14;_ring.rings=16;_ring.ring_segments=8
	if _core==null:
		_core=SphereMesh.new();_core.radius=.064;_core.height=.128;_core.radial_segments=12;_core.rings=6
	var ring:=MeshInstance3D.new()
	ring.mesh=_ring;ring.rotation.x=PI*.5
	ring.material_override=Materials.standard({"albedo_color":Color("c1d6dc"),"metallic":.7,"roughness":.22})
	add_child(ring)
	var core:=MeshInstance3D.new()
	core.mesh=_core;core.position.z=.035
	core.material_override=Materials.standard({"albedo_color":accent,"emission_enabled":true,"emission":accent,"emission_energy_multiplier":.35,"roughness":.25})
	add_child(core)
