# Nintendo Switch packaging

The Switch version loads some subtitle resources differently from the PC release. Installing translated `subtitle/*.dat` files as individual RomFS overrides can interfere with cinematic playback and scripted transitions. The Switch builder therefore uses an additional subtitle archive rather than loose subtitle files.

## Build contents

The Switch ZIP contains:

- `romfs/data300.cpk`, holding all 208 translated English subtitle DAT files.
- `exefs/e43525f22282a47785aaed2dfff6e2e94d82b129.ips`, selecting the additional subtitle archive before the original archive for **game update 1.0.2**.
- The other translated game files and Switch-specific UI resources as normal RomFS overrides.

The ZIP is ready for installation through the supported LayeredFS mod layout. Players do not need to unpack game files or repack the original `data009.cpk` or `data100.cpk`. No original game executable or CPK is included in the repository.

## Building and verification

The Rust application builds the subtitle CPK from the translated DAT files generated during the normal build. The source manifest (`assets/switch_subtitle_ids.tsv`) supplies the original archive identifiers; `assets/switch_subtitle_cpk_schema.json` describes the required CPK tables. The IPS file in `assets/` targets only the specified Switch build.

A build fails if any of the 208 translated subtitle DATs is missing. Before the Switch ZIP is finalized, the builder verifies that it contains `data300.cpk` and the appropriate IPS, and that loose subtitle files have not been included.

To verify an existing archive without starting the graphical application:

```sh
cargo run -- --verify-switch-zip path/to/Switch.zip
```

Or use the standalone checker:

```sh
python3 tools/check_switch_zip.py path/to/Switch.zip
```

The video from the title menu and early gameplay have been tested with the combined Switch patch. Other story sequences and other Switch executable builds require their own compatibility checks. The verifier checks package structure; it does not emulate the game.
