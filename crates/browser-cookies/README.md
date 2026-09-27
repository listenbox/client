# Native browser cookie reader

This private Rust executable uses `rookie-cookies` 0.6.0 for cookie discovery,
SQLite snapshots, operating-system key access, decryption and Safari decoding.
The catalogue adds Sweetcookie's browser brands that are absent from Rookie's
registry. It never implements a second decryption or database reader.

The sync engine builds it for the target platform and embeds the executable.
Import runs it in a private temporary directory over bounded JSON pipes; no
runtime installation, Go, Node, or garbage collector is required. Its separate
Cargo workspace keeps Rookie's Rusqlite 0.40 dependency out of the sync journal's
Refinery/Rusqlite dependency graph. Both Cargo lockfiles are checked in.

Only unpartitioned YouTube cookies from the selected profile's default Firefox
container leave the reader. The sync engine filters expiry, domain, path and
header syntax again, then saves the session atomically with private permissions.
Extraction, validation failures and cancellation before commit preserve the
previous session. Atomic replacement commits the new session; rereading the file
resolves a lost write acknowledgement. OS keychain prompts
can appear. Windows app-bound extraction is disabled, matching Sweetcookie's
limitations; use Firefox or scoped JSON when Chromium cookies cannot be read.

Pasted JSON is handled directly by the sync engine without invoking this process.
Native reader timeouts and cancellation terminate its process group on Unix;
the desktop task tracker waits for the importer before completing shutdown.

Sources: https://github.com/teng-lin/rookie-cookies and
https://github.com/steipete/sweetcookie (MIT; see THIRD-PARTY-NOTICES.txt).
