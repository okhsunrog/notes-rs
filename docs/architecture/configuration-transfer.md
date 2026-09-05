# Device configuration transfer

Settings → Configuration transfer exports a JSON file through the native file
picker. Android uses the Storage Access Framework and needs no root access or
broad storage permission. Export reads saved settings; unsaved form edits are not
included. Device tokens and appearance are opt-in.

```json
{
  "format": "tangleaf-configuration",
  "version": 1,
  "serverUrl": "https://notes.example/",
  "search": { "enabled": true, "trigger": "as_you_type", "rerank": true },
  "appearance": { "theme": "system", "palette": "iris" }
}
```

The optional `syncToken` field carries the device bearer token. Such files grant
server access: transfer them privately and delete the exported copy afterwards.
Provider API keys, notes, database identity, desktop window settings and local
navigation history are not exported. Server-owned AI configuration is shared
through the imported connection.

Import validates a bounded file (64 KiB), its version, URL and known fields before
showing a preview. The token stays in native memory and is never returned in the
preview. Cancel clears the staged import. Applying without a token preserves an
existing token only for the same server; a different server requires a token in
the file or the preview's password field. Selecting offline removes the device
token. Errors never echo invalid JSON fields or secret values.

Apply atomically saves the settings and checks the server with an authenticated
read-only request. An unreachable server does not discard the imported settings.
Restart when prompted to use changed credentials for sync and AI. Connecting may
download notes and upload local pending changes; importing configuration does not
replace or copy the local database.
