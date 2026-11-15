#pragma once
#include <gtk/gtk.h>
#include <stdint.h>
/* GTK-thread only. Multiple view identities; one process engine and GL context. */
GtkWidget *polar_graphics_gtk_create(const char *project, const char *scene, GError **error);
GtkWidget *polar_graphics_gtk_create_view(const char *project, const char *scene, const char *identity, GError **error);
GtkWidget *polar_graphics_gtk_create_pane(const char *project, const char *scene, const char *identity, GError **error);
gboolean polar_graphics_gtk_shutdown(GError **error);
typedef struct { uint64_t starts, mounts, frames, readbacks; GdkGLContext *context; } PolarGraphicsStats;
PolarGraphicsStats polar_graphics_gtk_stats(void);

uint64_t polar_graphics_gtk_view_frames(GtkWidget *widget);

void polar_graphics_gtk_retire(GtkWidget *widget);

typedef void (*PolarGraphicsHostEvent)(const char *json, void *context);
void polar_graphics_gtk_subscribe(GtkWidget *widget, PolarGraphicsHostEvent callback, void *context);
void polar_graphics_gtk_command(GtkWidget *widget, const char *action, const char *query);
