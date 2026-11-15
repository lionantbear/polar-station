#pragma once
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif
int polar_godot_start(int argc, char **argv, int width, int height, unsigned int fbo);
int polar_godot_frame(int width, int height, unsigned int fbo);
void polar_godot_stop(void);
void polar_godot_surface_scale(float scale);
int polar_godot_needs_frame(void);
int polar_godot_events_fd(void);
int64_t polar_godot_dispatch_events(void);
void polar_godot_pointer(int kind, double x, double y, double dx, double dy, int button, int pressed);
void polar_godot_pointer_event(int kind, double x, double y, double dx, double dy, int button, int pressed, unsigned int mask, unsigned int modifiers, int double_click);
void polar_godot_magnify(double x, double y, double factor, unsigned int modifiers);
void polar_godot_pan(double x, double y, double dx, double dy, unsigned int modifiers);
/* Native scroll phase flags: 1=begin, 2=end/cancel, 4=momentum. Legacy pan is unphased. */
void polar_godot_pan_event(double x, double y, double dx, double dy, unsigned int modifiers, unsigned int phase);
void polar_godot_key(unsigned int unicode, unsigned int keyval, int pressed, unsigned int modifiers);
uint64_t polar_godot_readbacks(void);
int polar_godot_check_state(void);
uint64_t polar_godot_view_create(const char *identity, const char *scene);
void polar_godot_view_configure(uint64_t id, int width, int height, float scale, int visible);
int polar_godot_view_needs_frame(uint64_t id);
void polar_godot_view_input(uint64_t id);
int polar_godot_step(unsigned int fbo);
int polar_godot_view_present(uint64_t id, int width, int height, unsigned int destination);
typedef void (*PolarGraphicsEvent)(const char *json, void *context);
void polar_godot_view_subscribe(uint64_t id, PolarGraphicsEvent callback, void *context);
void polar_godot_view_command(uint64_t id, const char *action, const char *query);
void polar_godot_view_destroy(uint64_t id);
/* RenderingDevice presentation. Normal frames remain on the GPU. */
void polar_godot_device_uuid(const unsigned char *uuid);
uint64_t polar_godot_native_device(void);
uint64_t polar_godot_native_queue(void);
uint64_t polar_godot_view_native_texture(uint64_t id);
void *polar_godot_export_create(uint64_t id,int width,int height,int *fd,uint64_t *size);
int polar_godot_export_semaphore(void *image,int ready);
int polar_godot_export_copy(void *image,uint64_t id);
void polar_godot_export_destroy(void *image);
/* Scene diagnostics for the desktop regression test. */
void polar_godot_camera(double *yaw, double *distance);
#ifdef __cplusplus
}
#endif
