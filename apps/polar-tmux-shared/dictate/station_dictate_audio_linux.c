/* Native PulseAudio ABI adapter, driven by GTK's existing GLib loop.
 * Also works with PipeWire's PulseAudio server. No polling, subprocess, audio
 * file or product state. Public ABI: freedesktop.org/software/pulseaudio/doxygen.
 * Symbols are loaded from the installed runtime; no second audio SDK/sysroot.
 */
#include "polar_station_desktop_client.h"
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

typedef void (*Notify)(void *, void *);
typedef void (*ReadNotify)(void *, size_t, void *);
/* Public pa_source_info prefix, verified against upstream introspect.h. */
typedef struct { const char *name; uint32_t index; const char *description; } SourceInfo;
typedef void (*SourceNotify)(void *, const SourceInfo *, int, void *);
typedef struct { int format; uint32_t rate; uint8_t channels; } SampleSpec;
typedef struct { uint32_t maxlength, tlength, prebuf, minreq, fragsize; } BufferAttributes;
typedef struct {
    void *(*pa_glib_mainloop_new)(void *);
    void *(*pa_glib_mainloop_get_api)(void *);
    void (*pa_glib_mainloop_free)(void *);
    void *(*pa_context_new)(void *, const char *);
    int (*pa_context_connect)(void *, const char *, int, const void *);
    void (*pa_context_set_state_callback)(void *, Notify, void *);
    int (*pa_context_get_state)(const void *);
    int (*pa_context_errno)(const void *);
    void (*pa_context_disconnect)(void *);
    void (*pa_context_unref)(void *);
    void *(*pa_context_get_source_info_list)(void *, SourceNotify, void *);
    void (*pa_operation_unref)(void *);
    const char *(*pa_strerror)(int);
    void *(*pa_stream_new)(void *, const char *, const SampleSpec *, const void *);
    void (*pa_stream_set_state_callback)(void *, Notify, void *);
    void (*pa_stream_set_read_callback)(void *, ReadNotify, void *);
    int (*pa_stream_get_state)(const void *);
    int (*pa_stream_connect_record)(void *, const char *, const void *, int);
    int (*pa_stream_peek)(void *, const void **, size_t *);
    int (*pa_stream_drop)(void *);
    size_t (*pa_stream_readable_size)(const void *);
    int (*pa_stream_disconnect)(void *);
    void (*pa_stream_unref)(void *);
} PulseApi;

struct PolarStationAudio {
    PulseApi api;
    void *pulse_library, *glib_library, *loop, *context, *stream;
    PolarStationAudioCallback callback;
    PolarStationAudioChunkCallback chunk;
    PolarStationAudioDeviceCallback devices;
    bool listing_started;
    char *device;
    void *callback_context;
    uint8_t *pcm;
    size_t length, capacity;
    bool stopped, ready, failed, limit;
    char error[192];
};

static void fail(PolarStationAudio *audio, const char *message) {
    if (audio->stopped || audio->failed) return;
    audio->failed = true;
    snprintf(audio->error, sizeof(audio->error), "%s", message ? message : "Microphone capture failed");
    audio->callback(audio->callback_context, POLAR_DICTATE_AUDIO_ERROR);
}
static void pulse_fail(PolarStationAudio *audio) {
    fail(audio, audio->api.pa_strerror(audio->api.pa_context_errno(audio->context)));
}
static void read_audio(void *stream, size_t available, void *userdata) {
    (void)available;
    PolarStationAudio *audio = userdata;
    if (audio->stopped || audio->failed || audio->limit) return;
    while (true) {
        size_t count = audio->api.pa_stream_readable_size(stream);
        if (count == (size_t)-1) { pulse_fail(audio); return; }
        if (!count) return;
        const void *data = NULL;
        if (audio->api.pa_stream_peek(stream, &data, &count) < 0) { pulse_fail(audio); return; }
        if (!count) return;
        size_t take = count < audio->capacity - audio->length ? count : audio->capacity - audio->length;
        if (audio->chunk) {
            size_t sent = 0;
            const uint8_t silence[640] = {0};
            while (sent < take) {
                size_t part = take - sent;
                if (part > sizeof(silence)) part = sizeof(silence);
                if (!audio->chunk(audio->callback_context, data ? (const uint8_t *)data + sent : silence, part)) {
                    audio->api.pa_stream_drop(stream);
                    fail(audio, "Audio storage stopped. Saved audio is available in Settings > Voice > Recordings.");
                    return;
                }
                sent += part;
            }
        } else {
            if (data) memcpy(audio->pcm + audio->length, data, take);
            else memset(audio->pcm + audio->length, 0, take);
        }
        audio->length += take;
        if (audio->api.pa_stream_drop(stream) < 0) { pulse_fail(audio); return; }
        if (audio->length == audio->capacity) {
            audio->limit = true;
            audio->callback(audio->callback_context, POLAR_DICTATE_AUDIO_LIMIT);
            return;
        }
    }
}
static void stream_state(void *stream, void *userdata) {
    PolarStationAudio *audio = userdata;
    if (audio->stopped || audio->failed) return;
    int state = audio->api.pa_stream_get_state(stream);
    if (state == 2 && !audio->ready) { /* PA_STREAM_READY */
        audio->ready = true;
        audio->callback(audio->callback_context, POLAR_DICTATE_AUDIO_READY);
    } else if (state == 3 || state == 4) { /* FAILED / TERMINATED */
        pulse_fail(audio);
    }
}
static void source_info(void *context, const SourceInfo *info, int eol, void *userdata) {
    (void)context;
    PolarStationAudio *audio = userdata;
    if (audio->stopped) return;
    if (eol < 0) { pulse_fail(audio); return; }
    if (audio->devices) audio->devices(audio->callback_context,
        info ? info->name : NULL, info ? info->description : NULL, eol > 0);
}
static void context_state(void *context, void *userdata) {
    PolarStationAudio *audio = userdata;
    if (audio->stopped || audio->failed) return;
    int state = audio->api.pa_context_get_state(context);
    if (state == 4 && audio->devices) {
        if (!audio->listing_started) {
            audio->listing_started = true;
            void *operation = audio->api.pa_context_get_source_info_list(context, source_info, audio);
            if (!operation) pulse_fail(audio); else audio->api.pa_operation_unref(operation);
        }
    } else if (state == 4 && !audio->stream) { /* PA_CONTEXT_READY */
        SampleSpec spec = { .format = 3, .rate = 16000, .channels = 1 }; /* PA_SAMPLE_S16LE */
        audio->stream = audio->api.pa_stream_new(context, "Dictate microphone", &spec, NULL);
        if (!audio->stream) { pulse_fail(audio); return; }
        audio->api.pa_stream_set_state_callback(audio->stream, stream_state, audio);
        audio->api.pa_stream_set_read_callback(audio->stream, read_audio, audio);
        /* Request 20 ms fragments so short hold-to-talk recordings do not
         * wait for the server's default multi-second recording buffer. */
        BufferAttributes attributes = { UINT32_MAX, UINT32_MAX, UINT32_MAX, UINT32_MAX, 640 };
        if (audio->api.pa_stream_connect_record(audio->stream, audio->device, &attributes, audio->device ? 512 /* DONT_MOVE */ : 0) < 0) pulse_fail(audio);
    } else if (state == 5 || state == 6) { /* FAILED / TERMINATED */
        pulse_fail(audio);
    }
}
#define LOAD(library, name) do { \
    void *symbol = dlsym(library, #name); \
    if (!symbol) goto unavailable; \
    memcpy(&audio->api.name, &symbol, sizeof(symbol)); \
} while (0)

PolarStationAudio *polar_station_dictate_audio_new(PolarStationAudioCallback callback, void *context, uint32_t maximum_ms) {
    if (!callback || maximum_ms == 0 || maximum_ms > 600000) return NULL;
    PolarStationAudio *audio = calloc(1, sizeof(*audio));
    if (!audio) return NULL;
    audio->callback = callback; audio->callback_context = context;
    audio->capacity = (size_t)maximum_ms * 32;
    audio->pcm = calloc(1, audio->capacity);
    if (!audio->pcm) goto unavailable;
    audio->pulse_library = dlopen("libpulse.so.0", RTLD_NOW | RTLD_LOCAL);
    audio->glib_library = dlopen("libpulse-mainloop-glib.so.0", RTLD_NOW | RTLD_LOCAL);
    if (!audio->pulse_library || !audio->glib_library) goto unavailable;
    LOAD(audio->glib_library, pa_glib_mainloop_new);
    LOAD(audio->glib_library, pa_glib_mainloop_get_api);
    LOAD(audio->glib_library, pa_glib_mainloop_free);
    LOAD(audio->pulse_library, pa_context_new);
    LOAD(audio->pulse_library, pa_context_connect);
    LOAD(audio->pulse_library, pa_context_set_state_callback);
    LOAD(audio->pulse_library, pa_context_get_state);
    LOAD(audio->pulse_library, pa_context_errno);
    LOAD(audio->pulse_library, pa_context_disconnect);
    LOAD(audio->pulse_library, pa_context_unref);
    LOAD(audio->pulse_library, pa_context_get_source_info_list);
    LOAD(audio->pulse_library, pa_operation_unref);
    LOAD(audio->pulse_library, pa_strerror);
    LOAD(audio->pulse_library, pa_stream_new);
    LOAD(audio->pulse_library, pa_stream_set_state_callback);
    LOAD(audio->pulse_library, pa_stream_set_read_callback);
    LOAD(audio->pulse_library, pa_stream_get_state);
    LOAD(audio->pulse_library, pa_stream_connect_record);
    LOAD(audio->pulse_library, pa_stream_peek);
    LOAD(audio->pulse_library, pa_stream_drop);
    LOAD(audio->pulse_library, pa_stream_readable_size);
    LOAD(audio->pulse_library, pa_stream_disconnect);
    LOAD(audio->pulse_library, pa_stream_unref);
    return audio;
unavailable:
    polar_station_dictate_audio_free(audio);
    return NULL;
}
#undef LOAD

bool polar_station_dictate_audio_start(PolarStationAudio *audio) {
    if (!audio || audio->loop || audio->stopped) return false;
    audio->loop = audio->api.pa_glib_mainloop_new(NULL);
    if (!audio->loop) return false;
    audio->context = audio->api.pa_context_new(audio->api.pa_glib_mainloop_get_api(audio->loop), "Polar Station Dictate");
    if (!audio->context) return false;
    audio->api.pa_context_set_state_callback(audio->context, context_state, audio);
    return audio->api.pa_context_connect(audio->context, NULL, 1 /* NOAUTOSPAWN */, NULL) >= 0;
}
void polar_station_dictate_audio_stop(PolarStationAudio *audio) {
    if (!audio || audio->stopped) return;
    audio->stopped = true;
    if (audio->stream) {
        audio->api.pa_stream_set_read_callback(audio->stream, NULL, NULL);
        audio->api.pa_stream_set_state_callback(audio->stream, NULL, NULL);
        audio->api.pa_stream_disconnect(audio->stream);
        audio->api.pa_stream_unref(audio->stream);
        audio->stream = NULL;
    }
    if (audio->context) {
        audio->api.pa_context_set_state_callback(audio->context, NULL, NULL);
        audio->api.pa_context_disconnect(audio->context);
        audio->api.pa_context_unref(audio->context);
        audio->context = NULL;
    }
    if (audio->loop) { audio->api.pa_glib_mainloop_free(audio->loop); audio->loop = NULL; }
}
const uint8_t *polar_station_dictate_audio_bytes(const PolarStationAudio *audio, size_t *length) {
    if (!length) return NULL;
    *length = audio && !audio->chunk ? audio->length - audio->length % 2 : 0;
    return audio ? audio->pcm : NULL;
}
const char *polar_station_dictate_audio_error(const PolarStationAudio *audio) {
    return audio && audio->error[0] ? audio->error : "Microphone capture could not start";
}
void polar_station_dictate_audio_free(PolarStationAudio *audio) {
    if (!audio) return;
    polar_station_dictate_audio_stop(audio);
    if (audio->pcm) {
        volatile uint8_t *bytes = audio->pcm;
        for (size_t i = 0; i < audio->capacity; ++i) bytes[i] = 0;
        free(audio->pcm);
    }
    if (audio->glib_library) dlclose(audio->glib_library);
    if (audio->pulse_library) dlclose(audio->pulse_library);
    free(audio->device);
    free(audio);
}

void polar_station_dictate_audio_set_chunk_callback(PolarStationAudio *audio, PolarStationAudioChunkCallback callback) {
    if (!audio || audio->context || audio->length || !callback) return;
    audio->chunk = callback;
    free(audio->pcm); audio->pcm = NULL;
}

void polar_station_dictate_audio_set_devices_callback(PolarStationAudio *audio, PolarStationAudioDeviceCallback callback) {
    if (!audio || audio->context) return;
    audio->devices = callback;
}
bool polar_station_dictate_audio_set_device(PolarStationAudio *audio, const char *name) {
    if (!audio || audio->context || (name && strlen(name) > 512)) return false;
    char *copy = NULL;
    if (name && name[0]) {
        const size_t length = strlen(name) + 1;
        copy = malloc(length);
        if (copy) memcpy(copy, name, length);
    }
    if (name && name[0] && !copy) return false;
    free(audio->device); audio->device = copy; return true;
}
