//! ferro std-profile binary entry point.
//!
//! This is the Faz 0 skeleton: it only reports build identity. The event-loop
//! transport (mio), filesystem asset/config sources, and request handling are
//! wired up in later phases (see `plan.md`, Faz 3 onward).

fn main() {
    println!("ferro {}", ferro_core::VERSION);
}
