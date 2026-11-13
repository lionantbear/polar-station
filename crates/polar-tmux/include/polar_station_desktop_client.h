#ifndef POLAR_STATION_DESKTOP_CLIENT_H
#define POLAR_STATION_DESKTOP_CLIENT_H

#include <stdbool.h>
#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Stages: 1 entry, 2 window presented, 3 focus requested, 4 focus acquired,
 * 5 frame presented. Optional identity is ASCII, at most 128 bytes. */
void polar_station_trace_native(uint32_t stage, const uint8_t *identity, size_t length, uint64_t generation);
void polar_station_trace_native_grid(uint32_t stage, uint64_t key, uint32_t columns, uint32_t rows, uint32_t pixel_width, uint32_t pixel_height);
void polar_station_trace_presentation(uint64_t key, int64_t frame_counter, int64_t presentation_us);

/* Shared local Station action catalog. Strings have static lifetime.
 * platform: 1 macOS physical keycode, 2 Linux normalized GDK keyval. */
typedef struct StationShortcutView {
    uint32_t action;
    uint8_t number;
    bool repeatable;
    const char *key;
    const char *label;
} StationShortcutView;
typedef struct StationShortcutHintContext { uint32_t panes; uint32_t tabs; bool idea; } StationShortcutHintContext;
size_t polar_station_shortcut_hint_count(StationShortcutHintContext);
StationShortcutView polar_station_shortcut_hint_at(StationShortcutHintContext, size_t index);
size_t polar_station_shortcut_count(void);
char *polar_station_resource_image_path(const uint8_t *bytes, size_t len);
char *polar_station_resource_file_path(const uint8_t *bytes, size_t len);
bool polar_station_file_input_valid(const uint8_t *bytes, size_t len);
size_t polar_station_resource_file_rule_count(void);
const char *polar_station_resource_file_rule_at(size_t index);
typedef struct ResourceFileOpenHandle ResourceFileOpenHandle;
ResourceFileOpenHandle *polar_station_resource_file_open_new(void);
char *polar_station_resource_file_open_dispatch(ResourceFileOpenHandle *, const uint8_t *, size_t);
void polar_station_resource_file_open_free(ResourceFileOpenHandle *);
uint32_t polar_station_shortcut_lookup(uint32_t platform, uint32_t key);
StationShortcutView polar_station_shortcut_at(size_t index);

typedef struct DesktopClientHandle DesktopClientHandle;
/* Native LGP state is shared Rust, never a TUI. UI-thread ownership: dispatch
 * and free must not race; returned strings use desktop_string_free. */
typedef struct LgpClientHandle LgpClientHandle;
LgpClientHandle *polar_station_input_new(void);
char *polar_station_input_dispatch(LgpClientHandle *, const uint8_t *, size_t);
char *polar_station_input_dispatch_at(LgpClientHandle *, const uint8_t *, size_t, uint64_t now_ms);
void polar_station_input_free(LgpClientHandle *);
LgpClientHandle *polar_station_lgp_client_new(void);
char *polar_station_lgp_client_dispatch(LgpClientHandle *, const uint8_t *, size_t);
void polar_station_lgp_client_free(LgpClientHandle *);
typedef void (*PolarStationDesktopWakeFn)(void *context);

/* All strings have static lifetime; callers must not free or modify them. */
typedef struct PolarStationRecoveryView {
    bool can_submit;
    bool local_enabled;
    const char *status;
    const char *save_label;
    const char *host_label;
    const char *local_label;
    const char *destination_label;
} PolarStationRecoveryView;

/* Remote SSH state never gates native Local entry or Host draft editing. */
PolarStationRecoveryView polar_station_recovery_view(
    bool connected, bool target_known, bool pending);

DesktopClientHandle *polar_station_desktop_client_new(
    const char *config_json,
    PolarStationDesktopWakeFn wake,
    void *wake_context
);
bool polar_station_desktop_client_start(DesktopClientHandle *handle);
bool polar_station_desktop_client_send_json(
    DesktopClientHandle *handle,
    const char *frame_json
);
/* 0 accepted; 1 not ready; 2 queue full; 3 unsupported; 4 invalid; 5 unavailable. */
uint32_t polar_station_desktop_client_send_json_status(DesktopClientHandle *handle, const char *frame_json);
uint64_t polar_station_desktop_client_generation(DesktopClientHandle *handle);
void polar_station_desktop_client_cancel_request(DesktopClientHandle *handle, uint64_t generation, const char *request_id);
char *polar_station_terminal_status_unavailable_json(void);
char *polar_station_desktop_client_pop_event_json(DesktopClientHandle *handle);
bool polar_station_desktop_client_session_json(DesktopClientHandle *handle, const char *request_json);

bool polar_station_desktop_client_native_state_json(DesktopClientHandle *handle, const char *request_json);
bool polar_station_desktop_client_send_content_json(
    DesktopClientHandle *handle,
    const char *request_json
);
bool polar_station_desktop_client_is_ready(DesktopClientHandle *handle);
bool polar_station_desktop_client_has_pending(DesktopClientHandle *handle);
bool polar_station_desktop_client_retry_now(DesktopClientHandle *handle);
void polar_station_desktop_client_stop(DesktopClientHandle *handle);
void polar_station_desktop_client_free(DesktopClientHandle *handle);
/* Fn on macOS; Control + Super on Linux. Strings have static lifetime. */
typedef struct StationDictateShortcut {
    uint32_t mac_keycode;
    uint32_t linux_keyval;
    const char *label;
    const char *mac_label;
    const char *linux_label;
    const char *xdg_trigger;
} StationDictateShortcut;
StationDictateShortcut polar_station_dictate_shortcut(void);
typedef struct StationShortcutGesture StationShortcutGesture;
StationShortcutGesture *polar_station_shortcut_gesture_new(void);
uint32_t polar_station_shortcut_gesture_event(StationShortcutGesture *, uint32_t event, bool recording, bool can_start, bool can_use_original);
void polar_station_shortcut_gesture_free(StationShortcutGesture *);
typedef struct DictateHandle DictateHandle;
DictateHandle *polar_station_dictate_new(void);
char *polar_station_dictate_dispatch(DictateHandle *handle, const uint8_t *bytes, size_t length);
void polar_station_dictate_free(DictateHandle *handle);
/* Blocking speech calls belong on a background thread. Keep the handle alive
 * until all calls return. Returned strings use desktop_string_free. */
typedef struct DictateSpeech DictateSpeech;
DictateSpeech *polar_station_dictate_speech_new(void);
char *polar_station_dictate_local_wav(const DictateSpeech *handle, const uint8_t *pcm, size_t length);
char *polar_station_dictate_service_request(const DictateSpeech *handle, const uint8_t *json, size_t length);
bool polar_station_dictate_speech_append(const DictateSpeech *handle, const uint8_t *id, size_t id_length, const uint8_t *pcm, size_t pcm_length);
/* Reserve synchronously before dispatching a worker; every successful reserve
 * requires exactly one transcribe call, even if cancelled before it starts. */
bool polar_station_dictate_speech_finish(const DictateSpeech *handle, const uint8_t *id, size_t length);
bool polar_station_dictate_speech_reserve(const DictateSpeech *handle, const uint8_t *id, size_t length);
char *polar_station_dictate_speech_transcribe(const DictateSpeech *handle,
    const uint8_t *id, size_t id_length, const uint8_t *pcm, size_t pcm_length);
void polar_station_dictate_speech_cancel(const DictateSpeech *handle, const uint8_t *id, size_t length);
typedef void (*PolarStationDictateProgress)(void *context, const uint8_t *id, size_t length, uint64_t saved_samples, const uint8_t *preview, size_t preview_length);
void polar_station_dictate_speech_set_progress(const DictateSpeech *handle, PolarStationDictateProgress callback, void *context);
typedef void (*PolarStationDictateHistory)(void *context, const uint8_t *json, size_t length);
void polar_station_dictate_speech_set_history(const DictateSpeech *handle, PolarStationDictateHistory callback, void *context);
void polar_station_dictate_speech_free(DictateSpeech *handle);
#ifdef __linux__
/* GLib UI thread only. Stop before reading PCM. Callbacks post notifications;
 * defer stop/free until the current GLib dispatch returns. */
typedef struct PolarStationAudio PolarStationAudio;
enum { POLAR_DICTATE_AUDIO_READY = 0, POLAR_DICTATE_AUDIO_LIMIT = 1, POLAR_DICTATE_AUDIO_ERROR = 2 };
typedef void (*PolarStationAudioCallback)(void *context, int event);
typedef void (*PolarStationAudioDeviceCallback)(void *context, const char *id, const char *label, bool done);
void polar_station_dictate_audio_set_devices_callback(PolarStationAudio *audio, PolarStationAudioDeviceCallback callback);
bool polar_station_dictate_audio_set_device(PolarStationAudio *audio, const char *name);
typedef bool (*PolarStationAudioChunkCallback)(void *context, const uint8_t *pcm, size_t length);
void polar_station_dictate_audio_set_chunk_callback(PolarStationAudio *audio, PolarStationAudioChunkCallback callback);
PolarStationAudio *polar_station_dictate_audio_new(PolarStationAudioCallback callback, void *context, uint32_t maximum_ms);
bool polar_station_dictate_audio_start(PolarStationAudio *audio);
void polar_station_dictate_audio_stop(PolarStationAudio *audio);
const uint8_t *polar_station_dictate_audio_bytes(const PolarStationAudio *audio, size_t *length);
const char *polar_station_dictate_audio_error(const PolarStationAudio *audio);
void polar_station_dictate_audio_free(PolarStationAudio *audio);
#endif

typedef struct SessionEditorHandle SessionEditorHandle;
SessionEditorHandle *polar_station_session_editor_new(void);
char *polar_station_session_editor_dispatch(SessionEditorHandle *handle, const uint8_t *bytes, size_t len);
void polar_station_session_editor_free(SessionEditorHandle *handle);
typedef struct ConnectionsHandle ConnectionsHandle;
ConnectionsHandle *polar_station_connections_new(void);
char *polar_station_connections_dispatch(ConnectionsHandle *handle, const uint8_t *bytes, size_t len);
void polar_station_connections_free(ConnectionsHandle *handle);
typedef struct AttentionHandle AttentionHandle;
AttentionHandle *polar_station_attention_new(void);
char *polar_station_attention_dispatch(AttentionHandle *, const uint8_t *, size_t);
void polar_station_attention_free(AttentionHandle *);
char *polar_station_agent_catalog_project(const uint8_t *bytes, size_t len);
char *polar_station_workspace_lifecycle(const uint8_t *bytes, size_t len);
void polar_station_desktop_string_free(char *value);

#ifdef __cplusplus
}
#endif

#endif
