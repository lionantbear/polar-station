# Polar Station

Polar Station is a native workstation for terminals and coding agents on
macOS and Linux. Organize local and SSH workspaces, work with Codex, and use
voice input alongside your terminal.

## Download and install

Get the [latest release](https://github.com/lionantbear/polar-station/releases).
The current downloadable trial is for **Apple Silicon Macs**; Linux source is
included. See the release page for verified platforms and known limitations.

Open the DMG and drag Polar Station to Applications. The trial is ad-hoc signed
and not notarized; follow its release-page instructions for the first launch.

## Getting started

Create or select a local or SSH workspace. Use Settings to configure coding
agents and voice input. The selected target's tmux can be installed by Polar
Station. Codex account login and voice-service credentials remain yours to set
up. macOS asks for microphone permission when recording is first requested.

The iPhone Companion is currently unavailable in this public build.

## Build and contribute

Run `python3 build-macos.py --help` or `python3 build-linux.py --help`.
See [macOS](BUILD_MACOS.md) or [Linux](BUILD_LINUX.md) prerequisites and commands.
Both entries compile editable source, including ZIP downloads, using pinned
Ghostty and Godot sources. Export and release tools are maintained separately.

Report problems in [Issues](https://github.com/lionantbear/polar-station/issues)
with your version, operating system and steps to reproduce.

## License

First-party code is licensed under [AGPL-3.0-only](LICENSE). Third-party
components retain their own terms; see [third-party notices](THIRD_PARTY_NOTICES.md).
Binary releases include full notices and source/rebuild directions.
