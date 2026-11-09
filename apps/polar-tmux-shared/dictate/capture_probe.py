"""Opt-in local microphone probe; keeps PCM only in memory, never uploads it."""
import argparse
import ctypes as c
import json


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--library", required=True)
    args = parser.parse_args()
    audio = c.CDLL(args.library)
    glib = c.CDLL("libglib-2.0.so.0")
    event_fn = c.CFUNCTYPE(None, c.c_void_p, c.c_int)
    timeout_fn = c.CFUNCTYPE(c.c_int, c.c_void_p)
    glib.g_main_loop_new.argtypes = [c.c_void_p, c.c_int]
    glib.g_main_loop_new.restype = c.c_void_p
    for name in ["g_main_loop_run", "g_main_loop_quit", "g_main_loop_unref"]:
        getattr(glib, name).argtypes = [c.c_void_p]
    glib.g_timeout_add.argtypes = [c.c_uint, timeout_fn, c.c_void_p]
    glib.g_timeout_add.restype = c.c_uint
    glib.g_source_remove.argtypes = [c.c_uint]
    audio.polar_station_dictate_audio_new.argtypes = [event_fn, c.c_void_p, c.c_uint32]
    audio.polar_station_dictate_audio_new.restype = c.c_void_p
    for name in ["start", "stop", "free", "error"]:
        getattr(audio, "polar_station_dictate_audio_" + name).argtypes = [c.c_void_p]
    audio.polar_station_dictate_audio_start.restype = c.c_bool
    audio.polar_station_dictate_audio_error.restype = c.c_char_p
    audio.polar_station_dictate_audio_bytes.argtypes = [c.c_void_p, c.POINTER(c.c_size_t)]
    audio.polar_station_dictate_audio_bytes.restype = c.c_void_p
    loop = glib.g_main_loop_new(None, 0)
    events = []

    @event_fn
    def event(_, kind):
        events.append(kind)
        if kind in (1, 2):
            glib.g_main_loop_quit(loop)

    @timeout_fn
    def timeout(_):
        events.append("timeout")
        glib.g_main_loop_quit(loop)
        return 0

    handle = audio.polar_station_dictate_audio_new(event, None, 300)
    if not handle:
        raise RuntimeError("Native audio library unavailable")
    timer = 0
    try:
        if not audio.polar_station_dictate_audio_start(handle):
            raise RuntimeError(audio.polar_station_dictate_audio_error(handle).decode())
        timer = glib.g_timeout_add(5000, timeout, None)
        glib.g_main_loop_run(loop)
        audio.polar_station_dictate_audio_stop(handle)
        length = c.c_size_t()
        audio.polar_station_dictate_audio_bytes(handle, c.byref(length))
        result = {"sample_rate": 16000, "channels": 1, "format": "s16le", "bytes": length.value,
                  "events": events, "passed": events == [0, 1] and length.value == 9600}
        if 2 in events:
            result["error"] = audio.polar_station_dictate_audio_error(handle).decode()
        print(json.dumps(result))
        return 0 if result["passed"] else 1
    finally:
        if timer and "timeout" not in events:
            glib.g_source_remove(timer)
        audio.polar_station_dictate_audio_free(handle)
        glib.g_main_loop_unref(loop)


if __name__ == "__main__":
    raise SystemExit(main())
