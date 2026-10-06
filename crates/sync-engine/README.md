# Listenbox sync engine

Shared Rust runtime for the [CLI](../cli/README.md) and
[desktop app](../desktop/README.md). It owns authentication, API access,
YouTube sessions, media preparation, and resumable synchronization.

## Shared YouTube cookie jar

**Production desktop and CLI reuse one cookie jar for the same OS user.**
Both resolve the same profile and use `youtube-cookies/jar.json` beneath it.
Neither application keeps a separate copy.

Seed the jar with a Netscape `cookies.txt` export through
`listenbox youtube-cookies import /path/to/cookies.txt`, or paste the complete
export into the desktop's **Settings → YouTube** and choose **Save cookies**.
See the [CLI cookie instructions](../cli/README.md#seed-youtube-cookies) for the
export guide and commands.

The Netscape file is an initial seed. After importing it, the engine reads the
saved jar for YouTube requests and persists cookie updates from YouTube
responses. The original export can be deleted. A fresh import replaces the
session; `listenbox youtube-cookies remove` or **Remove cookies** in desktop
settings returns future requests from both apps to anonymous access.

The importer accepts UTF-8 Netscape exports starting with
`# Netscape HTTP Cookie File` or `# HTTP Cookie File`, followed by seven
tab-separated fields per cookie row. It keeps unexpired cookies for `youtube.com`
and its subdomains, including session cookies with an expiry of `0`.
Malformed exports and exports with no usable YouTube cookies fail without
replacing the saved session.

The sync engine owns persistence and coordination between processes:

- Requests read complete snapshots; writers use an OS file lock and atomically
  replace the saved jar.
- Generation and per-cookie revision checks prevent late responses from
  overwriting newer cookies or undoing an explicit replacement or removal.
- Cookies are sent only to matching YouTube HTTPS endpoints. They are not
  uploaded to the Listenbox API or sent to media hosts.

## Profiles

| Build / platform | Default profile |
| --- | --- |
| Production macOS | `~/Library/Application Support/Listenbox/` |
| Production Windows | `%LOCALAPPDATA%\Listenbox\` |
| Production Linux | `$XDG_DATA_HOME/listenbox/`, defaulting to `~/.local/share/listenbox/` |
| Debug, all platforms | `~/.cache/listenbox/dev/` (`~` is the user's home directory) |

`LISTENBOX_PROFILE_DIR` overrides the entire profile: configuration, credentials,
cookies, SQLite, and downloads. Set it to the same directory for both apps if
you override it; an empty value is rejected. `--config PATH` changes only the
configuration file and does not select a different cookie jar.

Use the production CLI to seed the production desktop. Debug builds default to
a separate profile, even when configured to contact production services.
