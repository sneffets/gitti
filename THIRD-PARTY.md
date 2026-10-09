# Third-party material

gitti's own code: the apps gitti and gitty are GPL-3.0-or-later (`COPYING`), gitti-core and
the dev crates MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) — see README.md. It
ships no third-party fonts, themes or images; text uses the system fonts.

- **Icons**: [Lucide](https://lucide.dev) (ISC; a few icons derived from Feather, MIT),
  via `gpui-kit-assets` plus the extra icons embedded in `crates/gitti-app/src/assets.rs`.
  Licence text: `licenses/LUCIDE.txt`.
- **UI framework**: `gpui-kit` and its gpui / gpui-component stack (Apache-2.0).
- **Syntax colours**: gpui-component's default dark syntax theme (Apache-2.0).
- **Rust crates**: every dependency's licence is in its `Cargo.toml` (see `Cargo.lock`).
  Release binaries must ship a generated licence list of all compiled-in crates
  (e.g. `cargo about generate`).

The app icon (`assets/icon/`) is gitti's own design.
