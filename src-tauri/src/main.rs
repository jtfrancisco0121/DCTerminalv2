// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Started by Claude Code as a terminal tab's status line: record the
    // usage it reports and exit, without starting the app.
    if dcterminalv2_lib::status_line::run_if_requested() {
        return;
    }
    dcterminalv2_lib::run()
}
