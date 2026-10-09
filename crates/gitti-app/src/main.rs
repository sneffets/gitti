// No console window on Windows (also none for every askpass helper invocation).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    gitti_app::run();
}
