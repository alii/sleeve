# Sleeve

A fast, good-looking app for your rekordbox library. Browse your tracks, analyse them, set hot cues and put playlists on a USB stick.

![Sleeve](screenshot.png)

## Get it

- **Mac:** download [Sleeve.dmg](https://github.com/alii/sleeve/releases/latest/download/Sleeve.dmg), open it and drag Sleeve into Applications.
- **Windows:** download [Sleeve-windows.zip](https://github.com/alii/sleeve/releases/latest/download/Sleeve-windows.zip), unzip it and open Sleeve.exe. The first time, Windows may say it protected your PC: click **More info**, then **Run anyway**.

A new version comes out every time the code changes. See [all releases](https://github.com/alii/sleeve/releases).

### Build it yourself

1. Install Rust from [rustup.rs](https://rustup.rs).
2. Get Sleeve and open it:
   ```sh
   git clone https://github.com/alii/sleeve
   cd sleeve
   cargo run --release
   ```

## Good to know

- To add songs, drag them or their folders from Finder onto Sleeve. Drop them on a playlist to add them there too. Sleeve analyses them straight away.
- To put a playlist on a USB stick, right-click it.
- Sleeve backs up your library before it changes anything.
- Close rekordbox first. Sleeve won't change your library while rekordbox is open.
- Songs stored in iCloud download when you play them.

## Licence

GPL-2.0-or-later. Built with [Rune](https://github.com/anomalyco/rune) and the [rbxport](https://github.com/chrisle/rbxport) crates.
