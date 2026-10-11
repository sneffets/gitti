# Third-party material

gitti's own code: the apps gitti and gitty are GPL-3.0-or-later (`COPYING`), gitti-core and
the dev crates MIT OR Apache-2.0 (`LICENSE-MIT`, `LICENSE-APACHE`) — see README.md. It
ships no third-party fonts, themes or images; text uses the system fonts.

- **Rust crates**: every crate compiled into gitti or gitty, with its version, licence and
  licence texts, is listed in
  [`licenses/THIRD-PARTY-LICENSES.txt`](licenses/THIRD-PARTY-LICENSES.txt) — generated
  from `Cargo.lock` by `cargo xtask licenses` (per binary and platform; a test fails when
  `Cargo.lock` changed without regenerating it). The same list is built into both apps
  (gitti: Help ▸ Third-Party Licences…, summary in Help ▸ About, `gitti --licenses`;
  gitty: `gitty --licenses`, palette ▸ Third-Party Licences…) and copied next to the binaries by the
  bundle / install scripts in `scripts/`.
- **Icons**: [Lucide](https://lucide.dev) (ISC; a few icons derived from Feather, MIT),
  via `gpui-kit-assets` plus the extra icons embedded in `crates/gitti-app/src/assets.rs`.
  Licence text: `licenses/LUCIDE.txt` (also part of the list above).
- **UI framework**: `gpui-kit` and its gpui / gpui-component stack (Apache-2.0), in the list
  above.
- **Syntax colours**: gpui-component's default dark syntax theme (Apache-2.0).
- **Standard licence texts**: crates that ship no licence file of their own are listed with
  the standard text of their licence (`LICENSE-MIT` without our copyright line,
  `LICENSE-APACHE`, `licenses/standard/CC0-1.0.txt`) and their authors as the copyright
  holders.

The app icon (`assets/icon/`) is gitti's own design.
