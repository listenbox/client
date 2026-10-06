# Listenbox CLI

Publish and manage podcasts on [Listenbox](https://listenbox.app/) from your
terminal, scripts, or any agent with a shell.

## Quick start

Sign in, find your podcast, and upload a recording as a draft:

```sh
listenbox login
listenbox shows list
listenbox episodes create --show field-notes \
  --title "What the river remembers" \
  --file ./episode-42.mp4 \
  --publication draft
```

Use `--publication publish` to publish once processing finishes. Draft is the
default. Successful uploads print the episode title and its Listenbox link.

## Command reference

Run `listenbox --help` or append `--help` to any command for its options.
Replace uppercase placeholders with your values; square brackets mark optional
arguments.

| Command | Purpose |
| --- | --- |
| `listenbox login` | Authorize through your browser. |
| `listenbox auth status` | Inspect the current authorization. |
| `listenbox auth logout` | Remove the credential shared with the desktop app. |
| `listenbox import [--slug SLUG] URL` | Create a podcast from RSS or a public YouTube video, playlist, or channel. |
| `listenbox shows list` | List accessible podcast slugs. |
| `listenbox shows create --title TITLE --slug SLUG --type TYPE --language LANGUAGE [--artwork FILE]` | Create an audio or video podcast. |
| `listenbox shows order --show SLUG [--episode ID]...` | Read feed order, or move the supplied episodes to the front in sequence. |
| `listenbox shows sync youtube --show SLUG [--watch]` | Sync the podcast's saved YouTube source. |
| `listenbox shows delete --show SLUG --yes` | Delete a podcast. |
| `listenbox episodes list --show SLUG [--limit N]` | List episode IDs across all pages. |
| `listenbox episodes create --show SLUG --title TITLE --file FILE [--description TEXT] [--publication MODE]` | Upload a recording as a draft or publish it when ready. |
| `listenbox episodes delete --episode ID --yes` | Delete an episode. |
| `listenbox members list [--show SLUG]` | List members and pending invitations. |
| `listenbox members invite --email EMAIL --role ROLE [--show SLUG]` | Invite a team or podcast member. |
| `listenbox members role --member USER_ID --role ROLE [--show SLUG]` | Change a member's access. |
| `listenbox members remove --member USER_ID --yes [--show SLUG]` | Remove a member. |
| `listenbox youtube-cookies import FILE` | Seed or replace the shared YouTube session from a Netscape export. |
| `listenbox youtube-cookies remove` | Remove the shared YouTube session. |

- `TYPE` is `audio` or `video`; `LANGUAGE` uses a code such as `en` or `en-US`.
  Artwork must be a square JPEG or PNG, 1400–3000 pixels per side.
- `MODE` is `draft` or `publish`. Uploads accept `.mp3`, `.m4a`, `.wav`,
  `.flac`, `.mp4`, `.m4v`, and `.mov`.
- `ROLE` is `read` or `write`. Omit `--show` to manage team membership.
- `--limit` controls the episode page size (1–500, default 100), not the total
  number of results.
- Podcast slugs use lowercase letters, numbers, and single hyphens. Episode IDs
  come from `episodes list`; user IDs come from `members list`.

## YouTube import and sync

```sh
listenbox import --slug field-notes 'https://www.youtube.com/playlist?list=PLAYLIST_ID'
listenbox import --slug channel-notes 'https://www.youtube.com/@your-channel/videos'
listenbox shows sync youtube --show field-notes
listenbox shows sync youtube --show field-notes --watch
```

Import creates a new podcast. To continue an existing import, sync its slug.
Synchronization requires an active paid plan; CLI YouTube imports create video
podcasts. Watch mode syncs immediately, then hourly in the foreground, until
SIGINT or SIGTERM. Interrupted sync work is saved for the next run.

A complete source scan can remove episodes whose videos were removed from the
source. Playlist sync also restores source order. See the
[YouTube import guide](https://listenbox.app/guides/import-youtube-as-a-podcast/)
for the publishing workflow.

## Seed YouTube cookies

If YouTube asks you to sign in, export your session as a Netscape `cookies.txt`
file using the
[private-session export guide](https://listenbox.app/guides/import-youtube-as-a-podcast/#export-youtube-cookies).
Keep the complete header and tab-separated rows; JSON and copied HTTP Cookie
headers are not accepted.

Import the file with the production CLI, then sync your existing podcast:

```sh
listenbox youtube-cookies import /path/to/cookies.txt
listenbox shows sync youtube --show field-notes
```

Importing cookies does not require a Listenbox login. The export seeds the
[sync engine's persistent cookie jar](../sync-engine/README.md#shared-youtube-cookie-jar);
it is not a file you need to supply on every run. You can delete the export
after a successful import. YouTube's subsequent cookie updates are saved
automatically.

**Production CLI and desktop share this jar for the same OS user.** Cookies
imported here are available to the desktop's next sync. Saving cookies in
**Settings → YouTube** makes them available to the CLI too. Importing a fresh
export replaces the session for both apps.

To return both apps to anonymous YouTube access:

```sh
listenbox youtube-cookies remove
```

Cookies stay on your computer and are sent only to YouTube. Keep exported
sessions private. If YouTube requests sign-in again, import a fresh export.

## Configuration

The CLI and desktop share their profile, including Listenbox authorization,
YouTube cookies, and resumable sync work. See the
[profile locations](../sync-engine/README.md#profiles) for each platform.

Both read `config.yaml` from that profile. The global `--config PATH` option
selects another configuration file without changing the profile:

```sh
listenbox --config /path/to/config.yaml shows list
```

Release defaults are:

```yaml
api_origin: https://v1.listenbox.app
dashboard_origin: https://web.listenbox.app
```

`LISTENBOX_PROFILE_DIR` overrides the entire profile. To keep sharing, both
apps must use the same directory. By default, debug builds use a separate
development profile, so a debug CLI does not seed the production apps.
