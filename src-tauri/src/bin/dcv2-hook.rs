//! `dcv2-hook`: the A2 hook forwarder (see `observe/forward.rs`). Kept
//! apart from the app library so it starts in milliseconds.

#[allow(dead_code)]
#[path = "../observe/forward.rs"]
mod forward;

fn main() {
    forward::run_from_env();
    std::process::exit(0);
}
