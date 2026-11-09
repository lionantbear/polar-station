/* Hardware-independent ABI-buffer tests. No microphone or provider call. */
#include <assert.h>
#include "station_dictate_audio_linux.c"

static const uint8_t input[] = { 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20 };
static int chunk, events[3], peek_error;
static size_t readable(const void *stream) { (void)stream; return chunk < 2 ? (chunk == 0 ? 16 : 20) : 0; }
static int peek(void *stream, const void **data, size_t *length) {
    (void)stream;
    *data = chunk == 0 ? NULL : input; /* first buffer is a Pulse hole */
    *length = chunk == 0 ? 16 : 20;
    return peek_error ? -1 : 0;
}
static int drop(void *stream) { (void)stream; ++chunk; return 0; }
static int error_code(const void *context) { (void)context; return -1; }
static const char *error_string(int code) { (void)code; return "fixture capture failed"; }
static void event(void *context, int value) { (void)context; assert(value >= 0 && value < 3); ++events[value]; }
static int ready_state(const void *stream) { (void)stream; return 2; }

static size_t streamed;
static bool stream_chunk(void *context, const uint8_t *pcm, size_t count) {
    (void)context; (void)pcm; streamed += count; return true;
}
static bool blocked_chunk(void *context, const uint8_t *pcm, size_t count) {
    (void)context; (void)pcm; (void)count; return false;
}
int main(void) {
    uint8_t storage[40]; memset(storage, 0xee, sizeof(storage));
    PolarStationAudio audio = {
        .api = { .pa_stream_readable_size = readable, .pa_stream_peek = peek,
            .pa_stream_drop = drop, .pa_context_errno = error_code, .pa_strerror = error_string,
            .pa_stream_get_state = ready_state },
        .callback = event, .pcm = storage, .capacity = 32,
    };
    stream_state(NULL, &audio); stream_state(NULL, &audio);
    assert(events[POLAR_DICTATE_AUDIO_READY] == 1);
    read_audio(NULL, 36, &audio);
    assert(chunk == 2 && audio.length == 32 && audio.limit);
    assert(events[POLAR_DICTATE_AUDIO_LIMIT] == 1);
    for (size_t i = 0; i < 16; ++i) assert(storage[i] == 0);
    assert(memcmp(storage + 16, input, 16) == 0);
    for (size_t i = 32; i < sizeof(storage); ++i) assert(storage[i] == 0xee);
    read_audio(NULL, 20, &audio);
    assert(events[POLAR_DICTATE_AUDIO_LIMIT] == 1 && audio.length == 32);
    audio.limit = false; audio.length = 0; chunk = 0; peek_error = 1;
    read_audio(NULL, 16, &audio); read_audio(NULL, 16, &audio);
    assert(events[POLAR_DICTATE_AUDIO_ERROR] == 1 && audio.length == 0);
    assert(strcmp(polar_station_dictate_audio_error(&audio), "fixture capture failed") == 0);
    audio.stopped = true; audio.failed = false; peek_error = 0;
    read_audio(NULL, 16, &audio);
    assert(audio.length == 0);
    assert(!polar_station_dictate_audio_start(NULL));
    assert(polar_station_dictate_audio_new(NULL, NULL, 30000) == NULL);
    assert(polar_station_dictate_audio_new(event, NULL, 600001) == NULL);
    audio.stopped = false; audio.failed = false; audio.limit = false; audio.chunk = stream_chunk;
    audio.length = 0; chunk = 0; read_audio(NULL, 36, &audio);
    assert(streamed == 32 && audio.length == 32 && audio.limit);
    size_t length = 999; polar_station_dictate_audio_bytes(&audio, &length); assert(length == 0);
    audio.limit = false; audio.length = 0; chunk = 0; audio.chunk = blocked_chunk;
    read_audio(NULL, 36, &audio); assert(audio.failed && audio.length == 0);
    puts("Dictate native audio: cap, silence holes, ready, failure and stopped capture passed");
    return 0;
}
