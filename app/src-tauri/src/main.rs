#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    speakmode_lib::desktop::run();
}
