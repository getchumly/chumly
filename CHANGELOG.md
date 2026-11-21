# Changelog

All notable changes to this project are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

Versions: client and server share one version number per release, but they do not
have to match to work together. Each release starts with what needs updating
(client, server or both); entries are marked `Client:`, `Server:` or `Both:`.
Incompatible protocol changes are rare and listed under "Compatibility"; even then
a client and a server one version step apart keep working.

## [Unreleased]

**Update:** client only (servers can stay).

### Added
- Client: preview of your own screen share. While sharing, a live thumbnail sits in the
  bottom-left block; a click opens it large in a "Your screen" window with fullscreen,
  the viewer list and **Stop sharing**. The preview is local: no extra traffic and no
  sound. It can be hidden (and shown again) from the thumbnail; the choice is remembered.
- Client: when the shared window is minimized, the preview warns that viewers see a
  frozen picture.

### Changed
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

[Unreleased]: ../../compare/v0.1.0...HEAD
[0.1.0]: ../../releases/tag/v0.1.0
