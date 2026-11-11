/* Native GTK host adapted from iTree-View 58bc62a. See IMPORT.md. */
#include "polar_graphics_gtk.h"
#include "polar_godot.h"
#include <epoxy/gl.h>
#include <glib-unix.h>
#include <unistd.h>

typedef struct { void *export_image; GLuint memory,texture,read_fbo,ready,done; int image_width,image_height; uint64_t callback_generation; PolarGraphicsHostEvent callback; void *callback_context; uint64_t view, presented; char *identity; GtkWidget *area; double x,y,zoom_scale; guint button_mask,tick; gboolean pressed, retired; GHashTable *keys; } Host;
static struct {
    GThread *owner;
    GdkGLContext *context;
    GHashTable *hosts;
    gint64 step_time;
    guint event_source, deadline_source;
    char *project,*scene;
    gboolean started,stopped;
    uint64_t starts,mounts,frames;
} engine;
/* Scene signals can run inside GtkGLArea.render. Post projections through the
 * existing GTK main loop so they cannot invalidate an in-progress snapshot. */
typedef struct { GtkWidget *widget; char *json; uint64_t generation; } HostDelivery;
static gboolean deliver_host_event(gpointer data) {
    HostDelivery *delivery=data;
    Host *host=g_object_get_data(G_OBJECT(delivery->widget),"polar-graphics-host");
    if (host && !host->retired && host->callback && host->callback_generation==delivery->generation)
        host->callback(delivery->json,host->callback_context);
    return G_SOURCE_REMOVE;
}
static void free_host_delivery(gpointer data) {
    HostDelivery *delivery=data;
    g_object_unref(delivery->widget);g_free(delivery->json);g_free(delivery);
}
static void scene_host_event(const char *json,void *context) {
    Host *host=context;
    if (host->retired || !host->callback) return;
    HostDelivery *delivery=g_new0(HostDelivery,1);
    delivery->widget=g_object_ref(host->area);delivery->json=g_strdup(json);
    delivery->generation=host->callback_generation;
    g_idle_add_full(G_PRIORITY_DEFAULT_IDLE,deliver_host_event,delivery,free_host_delivery);
}
static GQuark error_domain(void) { return g_quark_from_static_string("polar-graphics-gtk"); }
static gboolean on_owner(GError **error) {
    if (engine.owner && engine.owner != g_thread_self()) { g_set_error_literal(error,error_domain(),1,"Graphics must run on its GTK owner thread");return FALSE; }
    return TRUE;
}
// GTK input and unmap callbacks are outside GLArea.render. Other native
// surfaces may have made their own context current in the meantime.
static gboolean select_input(Host *host) {
    if (!engine.started || !host->view) return FALSE;
    gdk_gl_context_make_current(engine.context);
    polar_godot_view_input(host->view);
    return TRUE;
}
static void release_input(Host *host) {
    if (!select_input(host)) return;
    for (guint index=1;index<=3;index++) if (host->button_mask & (1u << (index-1))) {
        host->button_mask &= ~(1u << (index-1));
        polar_godot_pointer_event(1,host->x,host->y,0,0,index,0,host->button_mask,0,0);
    }
    GHashTableIter iter;gpointer key;
    g_hash_table_iter_init(&iter,host->keys);
    while(g_hash_table_iter_next(&iter,&key,NULL)) polar_godot_key(gdk_keyval_to_unicode(GPOINTER_TO_UINT(key)),GPOINTER_TO_UINT(key),0,0);
    g_hash_table_remove_all(host->keys);
}
static void focus_leave(GtkEventControllerFocus *controller,gpointer data) { (void)controller;release_input(data); }
static GdkGLContext *create_context(GtkGLArea *area,gpointer data) {
    (void)data;GError *error=NULL;
    if(!engine.context) {
        engine.context=gdk_display_create_gl_context(gtk_widget_get_display(GTK_WIDGET(area)),&error);
        if(engine.context) {
            gdk_gl_context_set_allowed_apis(engine.context,GDK_GL_API_GL);
            gdk_gl_context_set_required_version(engine.context,3,3);
            if(!gdk_gl_context_realize(engine.context,&error)) g_clear_object(&engine.context);
        }
    }
    if(error) {gtk_gl_area_set_error(area,error);g_clear_error(&error);return NULL;}
    return engine.context ? g_object_ref(engine.context) : NULL;
}
static void request_frame(Host *host) {
    if (!host->retired && gtk_widget_get_mapped(host->area)) gtk_gl_area_queue_render(GTK_GL_AREA(host->area));
}
static gboolean tick(GtkWidget *widget,GdkFrameClock *clock,gpointer data) {
    (void)clock;Host *host=data;
    if (!engine.started || host->retired || !gtk_widget_get_mapped(widget) || !polar_godot_view_needs_frame(host->view)) {
        host->tick=0;return G_SOURCE_REMOVE;
    }
    gtk_gl_area_queue_render(GTK_GL_AREA(widget));
    return G_SOURCE_CONTINUE;
}
static gboolean dispatch_events(gpointer unused) {
    (void)unused;
    if (engine.deadline_source) {g_source_remove(engine.deadline_source);engine.deadline_source=0;}
    if (!engine.started || engine.stopped) return G_SOURCE_REMOVE;
    gdk_gl_context_make_current(engine.context);
    int64_t delay=polar_godot_dispatch_events();
    if (engine.hosts) {
        GHashTableIter iter;gpointer value;
        g_hash_table_iter_init(&iter,engine.hosts);
        while (g_hash_table_iter_next(&iter,NULL,&value)) {
            Host *host=value;
            if (host->view && polar_godot_view_needs_frame(host->view)) request_frame(host);
        }
    }
    if (delay>=0) engine.deadline_source=g_timeout_add((guint)MIN(MAX(delay,1),G_MAXUINT),dispatch_events,NULL);
    return G_SOURCE_REMOVE;
}
static gboolean events_ready(gint fd,GIOCondition condition,gpointer unused) {
    (void)fd;(void)unused;
    if (condition & (G_IO_ERR|G_IO_HUP)) {engine.event_source=0;return G_SOURCE_REMOVE;}
    dispatch_events(NULL);return G_SOURCE_CONTINUE;
}
static gboolean key_pressed(GtkEventControllerKey *controller, guint keyval, guint keycode, GdkModifierType state, gpointer data) {
    (void)controller; (void)keycode; (void)data;
    if (!select_input(data)) return TRUE;
    request_frame((Host *)data);
    g_hash_table_add(((Host *)data)->keys, GUINT_TO_POINTER(keyval));
    polar_godot_key(gdk_keyval_to_unicode(keyval),keyval,1,state);
    return TRUE;
}
static void key_released(GtkEventControllerKey *controller, guint keyval, guint keycode, GdkModifierType state, gpointer data) {
    (void)controller; (void)keycode; (void)data;
    if (!select_input(data)) return;
    request_frame((Host *)data);
    g_hash_table_remove(((Host *)data)->keys, GUINT_TO_POINTER(keyval));
    polar_godot_key(gdk_keyval_to_unicode(keyval),keyval,0,state);
}
static void motion(GtkEventControllerMotion *controller, double x, double y, gpointer data) {
    (void)controller;
    Host *host=data;
    if (!select_input(host)) return;
    request_frame(host);
    polar_godot_pointer_event(0,x,y,(x-host->x),(y-host->y),0,0,host->button_mask,controller ? gtk_event_controller_get_current_event_state(GTK_EVENT_CONTROLLER(controller)) : 0,0);
    host->x=x; host->y=y;
}
static void pressed(GtkGestureClick *gesture, int count, double x, double y, gpointer data) {
    (void)gesture; (void)count;
    Host *host=data;
    if (!select_input(host)) return;
    request_frame(host);
    gtk_widget_grab_focus(host->area);
    guint button=gesture ? gtk_gesture_single_get_current_button(GTK_GESTURE_SINGLE(gesture)) : GDK_BUTTON_PRIMARY;
    host->pressed=button==GDK_BUTTON_PRIMARY;
    host->x=x; host->y=y;
    int index=button==GDK_BUTTON_SECONDARY?2:button==GDK_BUTTON_MIDDLE?3:1;
    host->button_mask |= 1u << (index-1);
    polar_godot_pointer_event(1,x,y,0,0,index,1,host->button_mask,gesture ? gtk_event_controller_get_current_event_state(GTK_EVENT_CONTROLLER(gesture)) : 0,count==2);
}
static void released(GtkGestureClick *gesture, int count, double x, double y, gpointer data) {
    (void)gesture; (void)count;
    Host *host=data;
    if (!select_input(host)) return;
    request_frame(host);
    host->pressed=FALSE;
    guint button=gesture ? gtk_gesture_single_get_current_button(GTK_GESTURE_SINGLE(gesture)) : GDK_BUTTON_PRIMARY;
    int index=button==GDK_BUTTON_SECONDARY?2:button==GDK_BUTTON_MIDDLE?3:1;
    host->button_mask &= ~(1u << (index-1));
    polar_godot_pointer_event(1,x,y,0,0,index,0,host->button_mask,gesture ? gtk_event_controller_get_current_event_state(GTK_EVENT_CONTROLLER(gesture)) : 0,0);
}
static gboolean scroll(GtkEventControllerScroll *controller, double dx, double dy, gpointer data) {

    Host *host=data;
    if (!select_input(host)) return TRUE;
    request_frame(host);
    if (controller && gtk_event_controller_scroll_get_unit(controller) == GDK_SCROLL_UNIT_SURFACE) {
        /* Preserve GTK content-motion convention; shared graphics converts scrolling units once. */
        polar_godot_pan(host->x,host->y,-dx * 0.03,-dy * 0.03,gtk_event_controller_get_current_event_state(GTK_EVENT_CONTROLLER(controller)));
        return TRUE;
    }
    if (dy == 0) return TRUE;
    int button=dy>0?5:4;
    unsigned int modifiers=controller ? gtk_event_controller_get_current_event_state(GTK_EVENT_CONTROLLER(controller)) : 0;
    polar_godot_pointer_event(3,host->x,host->y,0,dy,button,1,host->button_mask,modifiers,0);
    polar_godot_pointer_event(1,host->x,host->y,0,0,button,0,host->button_mask,modifiers,0);
    return TRUE;
}

static void zoom_begin(GtkGesture *gesture,GdkEventSequence *sequence,gpointer data) {
    (void)gesture;(void)sequence; ((Host*)data)->zoom_scale=1.0;
}
static void zoom_changed(GtkGestureZoom *gesture,double scale,gpointer data) {
    Host *host=data;
    if (!select_input(host) || scale<=0) return;
    request_frame(host);
    double x=host->x,y=host->y;
    gtk_gesture_get_bounding_box_center(GTK_GESTURE(gesture),&x,&y);
    polar_godot_magnify(x,y,scale/(host->zoom_scale>0?host->zoom_scale:1),gtk_event_controller_get_current_event_state(GTK_EVENT_CONTROLLER(gesture)));
    host->zoom_scale=scale;
}

/* Presentation allocation is per view; hidden Tabs retain only scene state. */
static void release_image(Host *host) {
    if (!host->export_image) return;
    gdk_gl_context_make_current(engine.context);
    glFinish(); // Ownership/lifetime boundary only, never a normal frame.
    if(host->read_fbo)glDeleteFramebuffers(1,&host->read_fbo);
    if(host->texture)glDeleteTextures(1,&host->texture);
    if(host->memory)glDeleteMemoryObjectsEXT(1,&host->memory);
    if(host->ready)glDeleteSemaphoresEXT(1,&host->ready);
    if(host->done)glDeleteSemaphoresEXT(1,&host->done);
    polar_godot_export_destroy(host->export_image);
    host->export_image=NULL;host->memory=host->texture=host->read_fbo=host->ready=host->done=0;
    host->image_width=host->image_height=0;
}
static gboolean present_image(Host *host,int width,int height,GLuint destination) {
    if (width!=host->image_width || height!=host->image_height) {
        release_image(host);
        int fd=-1;uint64_t size=0;
        host->export_image=polar_godot_export_create(host->view,width,height,&fd,&size);
        if (!host->export_image) return FALSE;
        glCreateMemoryObjectsEXT(1,&host->memory);GLint dedicated=GL_TRUE;
        glMemoryObjectParameterivEXT(host->memory,GL_DEDICATED_MEMORY_OBJECT_EXT,&dedicated);
        glImportMemoryFdEXT(host->memory,size,GL_HANDLE_TYPE_OPAQUE_FD_EXT,fd);
        if(glGetError()!=GL_NO_ERROR){close(fd);return FALSE;}
        glGenTextures(1,&host->texture);glBindTexture(GL_TEXTURE_2D,host->texture);
        glTexParameteri(GL_TEXTURE_2D,GL_TEXTURE_TILING_EXT,GL_OPTIMAL_TILING_EXT);
        glTexStorageMem2DEXT(GL_TEXTURE_2D,1,GL_RGBA8,width,height,host->memory,0);
        glGenFramebuffers(1,&host->read_fbo);glBindFramebuffer(GL_READ_FRAMEBUFFER,host->read_fbo);
        glFramebufferTexture2D(GL_READ_FRAMEBUFFER,GL_COLOR_ATTACHMENT0,GL_TEXTURE_2D,host->texture,0);
        if(glGetError()!=GL_NO_ERROR || glCheckFramebufferStatus(GL_READ_FRAMEBUFFER)!=GL_FRAMEBUFFER_COMPLETE)return FALSE;
        GLuint *semaphores[]={&host->ready,&host->done};
        for(int i=0;i<2;i++) {
            fd=polar_godot_export_semaphore(host->export_image,i==0);if(fd<0)return FALSE;
            glGenSemaphoresEXT(1,semaphores[i]);glImportSemaphoreFdEXT(*semaphores[i],GL_HANDLE_TYPE_OPAQUE_FD_EXT,fd);
            if(glGetError()!=GL_NO_ERROR){close(fd);return FALSE;}
        }
        host->image_width=width;host->image_height=height;
    }
    if(!polar_godot_export_copy(host->export_image,host->view))return FALSE;
    GLenum layout=GL_LAYOUT_GENERAL_EXT;
    glWaitSemaphoreEXT(host->ready,0,NULL,1,&host->texture,&layout);
    glBindFramebuffer(GL_READ_FRAMEBUFFER,host->read_fbo);glBindFramebuffer(GL_DRAW_FRAMEBUFFER,destination);
    GLboolean scissor=glIsEnabled(GL_SCISSOR_TEST),srgb=glIsEnabled(GL_FRAMEBUFFER_SRGB);
    glDisable(GL_SCISSOR_TEST);glDisable(GL_FRAMEBUFFER_SRGB);
    glBlitFramebuffer(0,height,width,0,0,0,width,height,GL_COLOR_BUFFER_BIT,GL_NEAREST);
    glSignalSemaphoreEXT(host->done,0,NULL,1,&host->texture,&layout);glFlush();
    if(scissor)glEnable(GL_SCISSOR_TEST);
    if(srgb)glEnable(GL_FRAMEBUFFER_SRGB);
    glBindFramebuffer(GL_FRAMEBUFFER,destination);
    return glGetError()==GL_NO_ERROR;
}
static gboolean render(GtkGLArea *area,GdkGLContext *context,gpointer data) {
    (void)context;Host *host=data;
    if(host->retired || engine.stopped || gtk_gl_area_get_error(area)) return TRUE;
    gtk_gl_area_attach_buffers(area);GLint fbo=0;glGetIntegerv(GL_FRAMEBUFFER_BINDING,&fbo);
    int scale=gtk_widget_get_scale_factor(GTK_WIDGET(area));
    int width=gtk_widget_get_width(GTK_WIDGET(area))*scale,height=gtk_widget_get_height(GTK_WIDGET(area))*scale;
    if(width<1 || height<1 || !fbo) return TRUE;
    if(!engine.started) {
        if(!epoxy_has_gl_extension("GL_EXT_memory_object_fd") || !epoxy_has_gl_extension("GL_EXT_semaphore_fd")) {
            GError *error=g_error_new_literal(error_domain(),9,"Vulkan GPU sharing is unavailable on this GTK graphics device");
            gtk_gl_area_set_error(area,error);g_error_free(error);engine.stopped=TRUE;return TRUE;
        }
        unsigned char uuid[16];glGetUnsignedBytei_vEXT(GL_DEVICE_UUID_EXT,0,uuid);polar_godot_device_uuid(uuid);
        char *args[]={"polar-graphics","--path",engine.project,"--display-driver","native_rd","--rendering-driver","vulkan","--rendering-method","mobile","--render-thread","safe","--audio-driver","Dummy","--disable-vsync","res://polar_graphics_host.tscn",NULL};
        if(!polar_godot_start((int)(sizeof(args)/sizeof(args[0])-1),args,width,height,(unsigned)fbo)) {
            GError *error=g_error_new_literal(error_domain(),2,"LibGodot initialization failed; restart the application");
            gtk_gl_area_set_error(area,error);g_error_free(error);engine.stopped=TRUE;return TRUE;
        }
        engine.started=TRUE;engine.starts++;
        g_message("POLAR_GRAPHICS renderer=vulkan method=mobile embedded=1");
        int events_fd=polar_godot_events_fd();
        if (events_fd>=0) engine.event_source=g_unix_fd_add(events_fd,G_IO_IN|G_IO_ERR|G_IO_HUP,events_ready,NULL);
    }
    if (!host->view) {
        host->view=polar_godot_view_create(host->identity,engine.scene);
        if (!host->view) {
            GError *error=g_error_new_literal(error_domain(),8,"Graphics view initialization failed");
            gtk_gl_area_set_error(area,error);g_error_free(error);return TRUE;
        }
        polar_godot_view_subscribe(host->view,host->callback ? scene_host_event : NULL,host);
        engine.step_time=0;
    }
    if (g_object_get_data(G_OBJECT(area),"polar-station-pane")) {
        GtkRoot *root=gtk_widget_get_root(GTK_WIDGET(area));
        if (root) {
            const char *scope=g_object_get_data(G_OBJECT(root),"polar-graphics-window");
            if (!scope) {
                g_object_set_data_full(G_OBJECT(root),"polar-graphics-window",g_uuid_string_random(),g_free);
                scope=g_object_get_data(G_OBJECT(root),"polar-graphics-window");
            }
            const char *bound=g_object_get_data(G_OBJECT(area),"polar-graphics-bound-window");
            if (g_strcmp0(bound,scope)) {
                polar_godot_view_command(host->view,"bind_window",scope);
                g_object_set_data_full(G_OBJECT(area),"polar-graphics-bound-window",g_strdup(scope),g_free);
            }
        }
    }
    polar_godot_view_configure(host->view,width,height,(float)scale,1);
    GdkFrameClock *clock=gtk_widget_get_frame_clock(GTK_WIDGET(area));
    gint64 frame_time=clock ? gdk_frame_clock_get_frame_time(clock) : g_get_monotonic_time();
    int result=0;
    if (engine.step_time!=frame_time) {
        result=polar_godot_step((unsigned)fbo);engine.step_time=frame_time;engine.frames++;
    }
    if (!result) {
        if (present_image(host,width,height,(unsigned)fbo)) host->presented++;
        else {
            GError *error=g_error_new_literal(error_domain(),10,"Vulkan GPU image presentation failed");
            gtk_gl_area_set_error(area,error);g_error_free(error);
            release_image(host);result=1;
        }
    }
    if(result!=0) {
        GError *error=g_error_new_literal(error_domain(),3,"Graphics scene stopped; restart the application");
        gtk_gl_area_set_error(area,error);g_error_free(error);
    }
    if (!host->tick && polar_godot_view_needs_frame(host->view)) host->tick=gtk_widget_add_tick_callback(GTK_WIDGET(area),tick,host,NULL);
    return TRUE;
}
static void unrealize(GtkWidget *widget,gpointer data) {
    Host *host=data;release_input(host);release_image(host);
    if (host->tick) {gtk_widget_remove_tick_callback(widget,host->tick);host->tick=0;}
    if (host->view) polar_godot_view_configure(host->view,MAX(1,gtk_widget_get_width(widget)),MAX(1,gtk_widget_get_height(widget)),1,0);
}
void polar_graphics_gtk_retire(GtkWidget *widget) {
    GCancellable *cancel=g_object_get_data(G_OBJECT(widget),"polar-file-dialog");
    if (cancel) g_cancellable_cancel(cancel);
    Host *host=g_object_get_data(G_OBJECT(widget),"polar-graphics-host");
    if (!host || host->retired) return;
    host->retired=TRUE;release_input(host);release_image(host);
    if(host->tick) {gtk_widget_remove_tick_callback(widget,host->tick);host->tick=0;}
    if(host->view) {
        gdk_gl_context_make_current(engine.context);
        polar_godot_view_destroy(host->view);host->view=0;
    }
    if(engine.hosts && g_hash_table_lookup(engine.hosts,host->identity)==host)
        g_hash_table_remove(engine.hosts,host->identity);
}
static void retired(gpointer data,GObject *object) {
    (void)object;Host *host=data;release_image(host);
    if(host->view) {
        gdk_gl_context_make_current(engine.context);
        polar_godot_view_destroy(host->view);
    }
    if(engine.hosts && g_hash_table_lookup(engine.hosts,host->identity)==host)
        g_hash_table_remove(engine.hosts,host->identity);
    g_free(host->identity);g_hash_table_unref(host->keys);g_free(host);
}
GtkWidget *polar_graphics_gtk_create_view(const char *project,const char *scene,const char *identity,GError **error) {
    if(!on_owner(error))return NULL;
    if(!project || !scene || !g_path_is_absolute(project) || !g_str_has_prefix(scene,"res://")) {
        g_set_error_literal(error,error_domain(),4,"An absolute resource directory and res:// scene are required");return NULL;
    }
    if(!identity || !*identity || (engine.hosts && g_hash_table_contains(engine.hosts,identity)) || engine.stopped) {g_set_error_literal(error,error_domain(),5,"Graphics already mounted or shut down");return NULL;}
    if(engine.project && (g_strcmp0(engine.project,project) || g_strcmp0(engine.scene,scene))) {g_set_error_literal(error,error_domain(),6,"Graphics project/scene is fixed for this process");return NULL;}
    if(!engine.project) {engine.project=g_strdup(project);engine.scene=g_strdup(scene);}
    engine.owner=g_thread_self();Host *host=g_new0(Host,1);engine.mounts++;
    if(!engine.hosts) engine.hosts=g_hash_table_new(g_str_hash,g_str_equal);
    host->identity=g_strdup(identity);g_hash_table_insert(engine.hosts,host->identity,host);
    host->keys=g_hash_table_new(g_direct_hash,g_direct_equal);
    host->area=gtk_gl_area_new();
    g_object_set_data(G_OBJECT(host->area),"polar-graphics-host",host);GtkGLArea *area=GTK_GL_AREA(host->area);
    gtk_gl_area_set_allowed_apis(area,GDK_GL_API_GL);gtk_gl_area_set_required_version(area,3,3);
    gtk_gl_area_set_has_depth_buffer(area,TRUE);gtk_gl_area_set_auto_render(area,FALSE);
    gtk_widget_set_hexpand(host->area,TRUE);gtk_widget_set_vexpand(host->area,TRUE);gtk_widget_set_focusable(host->area,TRUE);
    g_signal_connect(area,"create-context",G_CALLBACK(create_context),host);
    g_signal_connect(area,"render",G_CALLBACK(render),host);
    // Remapping a retained, auto-render-disabled Tab needs one visibility frame.
    g_signal_connect_swapped(area,"map",G_CALLBACK(gtk_gl_area_queue_render),area);
    g_signal_connect(area,"unrealize",G_CALLBACK(unrealize),host);
    g_signal_connect(area,"unmap",G_CALLBACK(unrealize),host);
    GtkGesture *click=gtk_gesture_click_new();gtk_gesture_single_set_button(GTK_GESTURE_SINGLE(click),0);
    g_signal_connect(click,"pressed",G_CALLBACK(pressed),host);g_signal_connect(click,"released",G_CALLBACK(released),host);gtk_widget_add_controller(host->area,GTK_EVENT_CONTROLLER(click));
    GtkEventController *moves=gtk_event_controller_motion_new();g_signal_connect(moves,"motion",G_CALLBACK(motion),host);gtk_widget_add_controller(host->area,moves);
    GtkGesture *zoom=gtk_gesture_zoom_new();
    g_signal_connect(zoom,"begin",G_CALLBACK(zoom_begin),host);
    g_signal_connect(zoom,"scale-changed",G_CALLBACK(zoom_changed),host);
    gtk_widget_add_controller(host->area,GTK_EVENT_CONTROLLER(zoom));
    GtkEventController *wheel=gtk_event_controller_scroll_new(GTK_EVENT_CONTROLLER_SCROLL_BOTH_AXES);g_signal_connect(wheel,"scroll",G_CALLBACK(scroll),host);gtk_widget_add_controller(host->area,wheel);
    GtkEventController *keys=gtk_event_controller_key_new();g_signal_connect(keys,"key-pressed",G_CALLBACK(key_pressed),host);g_signal_connect(keys,"key-released",G_CALLBACK(key_released),host);gtk_widget_add_controller(host->area,keys);
    GtkEventController *focus=gtk_event_controller_focus_new();g_signal_connect(focus,"leave",G_CALLBACK(focus_leave),host);gtk_widget_add_controller(host->area,focus);

    g_object_weak_ref(G_OBJECT(host->area),retired,host);
    return host->area;
}
GtkWidget *polar_graphics_gtk_create_pane(const char *project,const char *scene,const char *identity,GError **error) {
    GtkWidget *widget=polar_graphics_gtk_create_view(project,scene,identity,error);
    if (widget) g_object_set_data(G_OBJECT(widget),"polar-station-pane",GINT_TO_POINTER(1));
    return widget;
}
GtkWidget *polar_graphics_gtk_create(const char *project,const char *scene,GError **error) {
    return polar_graphics_gtk_create_view(project,scene,"legacy-view",error);
}
gboolean polar_graphics_gtk_shutdown(GError **error) {
    if(!on_owner(error))return FALSE;
    if(engine.hosts && g_hash_table_size(engine.hosts)) {g_set_error_literal(error,error_domain(),7,"Retire the graphics widget before shutdown");return FALSE;}
    if (engine.event_source) {g_source_remove(engine.event_source);engine.event_source=0;}
    if (engine.deadline_source) {g_source_remove(engine.deadline_source);engine.deadline_source=0;}
    if(engine.started) {gdk_gl_context_make_current(engine.context);polar_godot_stop();engine.started=FALSE;}
    g_clear_pointer(&engine.hosts,g_hash_table_unref);
    g_clear_object(&engine.context);g_clear_pointer(&engine.project,g_free);g_clear_pointer(&engine.scene,g_free);engine.stopped=TRUE;
    return TRUE;
}
PolarGraphicsStats polar_graphics_gtk_stats(void) {
    return (PolarGraphicsStats){engine.starts,engine.mounts,engine.frames,polar_godot_readbacks(),engine.context};
}

uint64_t polar_graphics_gtk_view_frames(GtkWidget *widget) {
    Host *host=g_object_get_data(G_OBJECT(widget),"polar-graphics-host");
    return host ? host->presented : 0;
}

void polar_graphics_gtk_subscribe(GtkWidget *widget, PolarGraphicsHostEvent callback, void *context) {
    Host *host=g_object_get_data(G_OBJECT(widget),"polar-graphics-host");
    if (!host || host->retired || !on_owner(NULL)) return;
    host->callback_generation++;host->callback=callback;host->callback_context=context;
    if (select_input(host)) polar_godot_view_subscribe(host->view,callback ? scene_host_event : NULL,host);
}
typedef struct { GWeakRef widget; uint64_t view; } FileChoice;
static void append_json_path(GString *json, const char *path) {
    g_string_append_c(json, '"');
    for (const unsigned char *p=(const unsigned char *)path; *p; p++) {
        if (*p=='"' || *p=='\\') g_string_append_c(json, '\\');
        if (*p<0x20) g_string_append_printf(json,"\\u%04x",*p);
        else g_string_append_c(json,*p);
    }
    g_string_append_c(json, '"');
}
static void files_chosen(GObject *dialog, GAsyncResult *result, gpointer data) {
    FileChoice *choice=data;
    GError *error=NULL;
    GListModel *files=gtk_file_dialog_open_multiple_finish(GTK_FILE_DIALOG(dialog),result,&error);
    GtkWidget *widget=g_weak_ref_get(&choice->widget);
    Host *host=widget ? g_object_get_data(G_OBJECT(widget),"polar-graphics-host") : NULL;
    if (files && host && !host->retired && host->view==choice->view) {
        GString *json=g_string_new("[");
        for (guint i=0;i<g_list_model_get_n_items(files);i++) {
            GFile *file=g_list_model_get_item(files,i);
            char *path=g_file_get_path(file);
            if (path) { if (json->len>1) g_string_append_c(json,',');append_json_path(json,path); }
            g_free(path);g_object_unref(file);
        }
        g_string_append_c(json,']');
        if (json->len>2) polar_graphics_gtk_command(widget,"load_files",json->str);
        g_string_free(json,TRUE);
    }
    if (error && !g_error_matches(error,GTK_DIALOG_ERROR,GTK_DIALOG_ERROR_DISMISSED) && !g_error_matches(error,GTK_DIALOG_ERROR,GTK_DIALOG_ERROR_CANCELLED))
        g_warning("Native Idea Tree file chooser: %s",error->message);
    g_clear_error(&error);g_clear_object(&files);
    if (widget) {g_object_set_data(G_OBJECT(widget),"polar-file-dialog",NULL);g_object_unref(widget);}
    g_weak_ref_clear(&choice->widget);g_free(choice);
}
static void choose_files(GtkWidget *widget, Host *host) {
    if (g_object_get_data(G_OBJECT(widget),"polar-file-dialog")) return;
    GtkRoot *root=gtk_widget_get_root(widget);
    GtkFileDialog *dialog=gtk_file_dialog_new();
    gtk_file_dialog_set_title(dialog,"Open Idea Trees");
    GCancellable *cancel=g_cancellable_new();
    g_object_set_data_full(G_OBJECT(widget),"polar-file-dialog",cancel,g_object_unref);
    FileChoice *choice=g_new0(FileChoice,1);g_weak_ref_init(&choice->widget,widget);choice->view=host->view;
    gtk_file_dialog_open_multiple(dialog,GTK_IS_WINDOW(root) ? GTK_WINDOW(root) : NULL,cancel,files_chosen,choice);
    g_object_unref(dialog);
}

typedef struct { GWeakRef widget; uint64_t view; gboolean save; gboolean configuration; char *identity; } PackageChoice;
static void package_chosen(GObject *dialog,GAsyncResult *result,gpointer data) {
    PackageChoice *choice=data;GError *error=NULL;
    GFile *file=choice->save ? gtk_file_dialog_save_finish(GTK_FILE_DIALOG(dialog),result,&error) : gtk_file_dialog_open_finish(GTK_FILE_DIALOG(dialog),result,&error);
    GtkWidget *widget=g_weak_ref_get(&choice->widget);
    Host *host=widget ? g_object_get_data(G_OBJECT(widget),"polar-graphics-host") : NULL;
    if (file && host && !host->retired && host->view==choice->view) {
        char *path=g_file_get_path(file);
        if (path && choice->save) {
            GString *json=g_string_new("{\"id\":");append_json_path(json,choice->identity);
            g_string_append(json,",\"destination\":");append_json_path(json,path);g_string_append_c(json,'}');
            polar_graphics_gtk_command(widget,"export_package",json->str);g_string_free(json,TRUE);
        } else if (path) polar_graphics_gtk_command(widget,choice->configuration ? "load_browsing_configuration" : "install_package",path);
        g_free(path);
    }
    if (error && !g_error_matches(error,GTK_DIALOG_ERROR,GTK_DIALOG_ERROR_DISMISSED) && !g_error_matches(error,GTK_DIALOG_ERROR,GTK_DIALOG_ERROR_CANCELLED)) g_warning("View package chooser: %s",error->message);
    g_clear_error(&error);g_clear_object(&file);
    if (widget) {g_object_set_data(G_OBJECT(widget),"polar-file-dialog",NULL);g_object_unref(widget);}
    g_weak_ref_clear(&choice->widget);g_free(choice->identity);g_free(choice);
}
static void choose_package(GtkWidget *widget,Host *host,gboolean save,const char *identity,gboolean configuration) {
    if (g_object_get_data(G_OBJECT(widget),"polar-file-dialog")) return;
    GtkFileDialog *dialog=gtk_file_dialog_new();
    gtk_file_dialog_set_title(dialog,configuration ? "Open Browsing Configuration" : (save ? "Export view package" : "Install trusted view package"));
    GCancellable *cancel=g_cancellable_new();g_object_set_data_full(G_OBJECT(widget),"polar-file-dialog",cancel,g_object_unref);
    PackageChoice *choice=g_new0(PackageChoice,1);g_weak_ref_init(&choice->widget,widget);choice->view=host->view;choice->save=save;choice->configuration=configuration;choice->identity=g_strdup(identity);
    GtkRoot *root=gtk_widget_get_root(widget);GtkWindow *parent=GTK_IS_WINDOW(root) ? GTK_WINDOW(root) : NULL;
    if (save) {gtk_file_dialog_set_initial_name(dialog,"view.polar-view.zip");gtk_file_dialog_save(dialog,parent,cancel,package_chosen,choice);}
    else gtk_file_dialog_open(dialog,parent,cancel,package_chosen,choice);
    g_object_unref(dialog);
}

void polar_graphics_gtk_command(GtkWidget *widget, const char *action, const char *query) {
    Host *host=g_object_get_data(G_OBJECT(widget),"polar-graphics-host");
    if (!host || host->retired || !on_owner(NULL) || !select_input(host)) return;
    if (!g_strcmp0(action,"open_files")) { choose_files(widget,host);return; }
    if (!g_strcmp0(action,"open_browsing_configuration")) {choose_package(widget,host,FALSE,"",TRUE);return;}
    if (!g_strcmp0(action,"install_view_package") || !g_strcmp0(action,"export_view_package")) {choose_package(widget,host,!g_strcmp0(action,"export_view_package"),query,FALSE);return;}
    polar_godot_view_command(host->view,action,query);
    if (!g_strcmp0(action,"open_files") || !g_strcmp0(action,"read")) gtk_widget_grab_focus(widget);
    request_frame(host);
    gtk_gl_area_queue_render(GTK_GL_AREA(widget));
}
