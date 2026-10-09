# README screenshots

`shoot.sh` regenerates every image in `docs/images` from a fixed state of a public example
repository ([VectorCraft](https://github.com/storytold/vectorcraft) at `c2711df`, MIT OR
Apache-2.0), so the README can follow UI changes with one command:

```sh
scripts/readme-shots/shoot.sh          # set up the demo, build, shoot gitti and gitty
scripts/readme-shots/shoot.sh gitty    # only the terminal shots (also: gitti, setup)
```

- macOS only (gitti's `shot:` script step); needs git, cargo, python3, sips and `cwebp`
  (`brew install webp`).
- The demo lives in `/tmp/gitti-demo` (`GITTI_SHOTS_DIR`). Its path is visible in the
  screenshots — keep it free of user names. gitti and gitty run with their own git identity
  (Jane Doe) and settings there; author e-mails in the demo's history are replaced by noreply
  addresses.
- gitti shots: the GUI's `GITTI_SCRIPT` (`crates/gitti-app/src/script.rs`), saved as lossless
  WebP at 2000 px. gitty shots: headless `GITTY_SCRIPT` (`crates/gitty/src/script.rs`), every
  screen row's `cells:Y` rendered as SVG by `term2svg.py`.
