// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    // Started by Git or ssh as their askpass program: pass the prompt to the running app and
    // print its answer, without starting an app of its own.
    if let Some(code) = stashpanda_lib::askpass::run_as_helper() {
        std::process::exit(code);
    }
    stashpanda_lib::run()
}
