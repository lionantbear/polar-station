extends RefCounted
## Native adapters supply Godot scrolling units. Consumers use content pixels.
## This conversion belongs to shared graphics, never a browser or theme.
static func content_pixels(delta:Vector2)->Vector2:
	return -delta*10.0
