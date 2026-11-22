# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Versions: client and server share one version number per release, but they do not
have to match to work together. Each release starts with what needs updating
(client, server or both); entries are marked `Client:`, `Server:` or `Both:`.
Incompatible protocol changes are rare and listed under "Compatibility"; even then
a client and a server one version step apart keep working.

## [Unreleased]

## [0.2.0] - 2026-11-22

**Update:** client only (servers can stay). Linux users: the client now needs PipeWire
(`libpipewire-0.3`), which desktop systems normally have.

### Added
- Client: **screen sharing on Linux.** With X11 (Linux Mint, Ubuntu on Xorg and similar):
  a monitor or a window, with the mouse pointer; a covered window is still shared. With
  Wayland (Ubuntu, Fedora, KDE): the system dialog asks what to share. No sound yet
  (Windows only for now).
- Client: **watching screen shares on Linux**, with their sound. Works with servers of any
  version.

### Changed
- Client: watching a screen share takes less CPU: the picture's colors are now converted by
  the graphics card, and 2.5 times less data goes to it per frame. If the graphics card
  cannot do this (very old OpenGL), the client converts on the CPU as before. Hovering the
  stats in the viewer window shows the time per frame of each stage.

### Fixed
- Client: screen sharing no longer makes the sharer's memory grow. Every share from a
  high-resolution monitor (e.g. 2560×1440) used to keep about 80 MB that was never given
  back, so starting and stopping a share several times pushed the client to 1 GB and more.
  Frames are also downscaled with less work: the sharer's CPU load dropped by about a fifth.

### Security
- Client: updated the `url` library (RUSTSEC-2026-0592, a crash on crafted `blob:` links;
  the client only uses it to open links in the browser).

## [0.1.2] - 2026-11-21

**Update:** client only (servers can stay). This is the last update you install by hand:
from this version on the client updates itself.

### Added
- Client: **Update and restart**. The client downloads the new version, checks its
  signature against the release key built into the client (and that it is the file of that
  version), replaces itself and restarts. Progress and Cancel while downloading. In a call,
  it asks before restarting ("You will be disconnected from …"); **Later** installs it on
  the next start. A file with a wrong signature is never installed; a network error or a
  folder without write access offers the manual download instead. Works on Windows and Linux.
- Client: "Updated to X" with What's new on the start screen after an update.

## [0.1.1] - 2026-11-21

**Update:** client only (servers can stay).

### Added
- Client: preview of your own screen share. While sharing, a live thumbnail sits in the
  bottom-left block; a click opens it large in a "Your screen" window with fullscreen,
  the viewer list and **Stop sharing**. The preview is local: no extra traffic and no
  sound. It can be hidden (and shown again) from the thumbnail; the choice is remembered.
- Client: when the shared window is minimized, the preview warns that viewers see a
  frozen picture.

### Changed
- Client: screen share bitrate doubled: 5 Mbit/s at 720p 30, 8 at 1080p 30, 12 at 1080p 60.
  Fast motion in games no longer turns to mush as quickly. The sharer needs that much
  upload speed; on a slow connection choose 720p.
- Client: in a narrow channel panel, **Disconnect** becomes an icon while sharing, so
  **Stop sharing** fits.

## [0.1.0] - 2026-11-20

Initial release.

- Voice over QUIC datagrams: Opus, RNNoise noise suppression, jitter buffer with FEC.
- Push-to-talk or voice activation, global hotkeys for mute and deafen.
- Channels with user limits, channel chat, private messages.
- Screen sharing with audio (H.264, Windows only); everyone sees who is watching.
- Event sounds in groups, each with its own switch and volume.
- Identity: a recovery code the client creates on first start; server rights are bound
  to the user's key.
- Roles as data: Owner, Admin and Moderator by default, custom roles with chosen rights;
  registered nicknames; kick, temporary and permanent bans; channel management from the
  client or the server console.
- Server password set from the client or the console, without a restart.
- Abuse protection: wrong-password limits per address and server-wide, connection
  limits per address, rate limits on chat, control messages and media.
- Self-signed TLS with certificate pinning on first connect.
- Server list with status, ping and online count before connecting.
- Update check against GitHub releases; client and server versions are agreed when
  connecting.
- Crash reports saved locally, nothing is sent anywhere.
- Portable mode, light and dark themes, English and Russian UI.
- Releases are signed with minisign.

[Unreleased]: ../../compare/v0.2.0...HEAD
[0.2.0]: ../../compare/v0.1.2...v0.2.0
[0.1.2]: ../../compare/v0.1.1...v0.1.2
[0.1.1]: ../../compare/v0.1.0...v0.1.1
[0.1.0]: ../../releases/tag/v0.1.0
