//! `embedded-server` — bare-metal (`no_std`) ferro web server for Cortex-M.
//!
//! This binary is the embedded profile: it links `ferro-core` (protocol logic)
//! and `ferro-embedded` (the smoltcp transport) into a freestanding firmware
//! image with a `cortex-m-rt` entry point, a heap allocator, and the document
//! root and configuration baked in with `include_bytes!`/`include_str!`.
//!
//! It is built standalone for `thumbv7em-none-eabi` (see `.cargo/config.toml`),
//! not as part of the workspace, because a `#![no_main]` firmware cannot link
//! for the host. The smoltcp device here is [`Loopback`], which stands in for a
//! board's Ethernet MAC: this proves the whole embedded stack assembles and
//! links, while serving real network traffic requires a hardware device driver
//! (or a QEMU machine that provides one).

#![no_std]
#![no_main]

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

// Pulled in for its linked symbols only: cortex-m provides the
// `critical-section` implementation the heap allocator needs; panic-halt
// provides the panic handler.
use cortex_m as _;
use cortex_m_rt::entry;
use embedded_alloc::LlffHeap as Heap;
use panic_halt as _;

use ferro_core::asset::EmbeddedAssets;
use ferro_core::config::Config;
use ferro_core::http::method::Method;
use ferro_core::http::request::Request;
use ferro_core::http::response::Response;
use ferro_core::http::status::StatusCode;
use ferro_core::router::{Params, Router};

use ferro_embedded::{serve_smoltcp, ConnState, StaticRouter};

use smoltcp::iface::{Config as IfaceConfig, Interface, SocketSet};
use smoltcp::phy::{Loopback, Medium};
use smoltcp::socket::tcp;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr};

/// Global allocator: ferro-core builds responses with alloc collections, so the
/// firmware needs a heap.
#[global_allocator]
static HEAP: Heap = Heap::empty();

/// Heap size; ample for the small request/response and socket buffer sizes.
const HEAP_SIZE: usize = 32 * 1024;

/// The document root, baked into the firmware image at compile time.
static ASSETS: &[(&str, &[u8])] = &[("index.html", include_bytes!("../assets/index.html"))];

/// The server configuration, baked in as JSON and parsed on boot.
const CONFIG: &str = include_str!("../assets/config.json");

/// Liveness endpoint, mirroring the std profile's `/api/health`.
fn health(_req: &Request, _p: &Params) -> Response {
    Response::json(StatusCode::OK, "{\"status\":\"ok\"}")
}

#[entry]
fn main() -> ! {
    // The heap must be live before any alloc::* type is constructed.
    {
        use core::mem::MaybeUninit;
        static mut HEAP_MEM: [MaybeUninit<u8>; HEAP_SIZE] = [MaybeUninit::uninit(); HEAP_SIZE];
        // SAFETY: runs exactly once at startup before the first allocation;
        // HEAP_MEM is private to this block and never aliased elsewhere.
        unsafe { HEAP.init(core::ptr::addr_of_mut!(HEAP_MEM) as usize, HEAP_SIZE) }
    }

    // Parse the baked-in configuration. A malformed embedded config is a build
    // bug, so fall back to defaults rather than panicking the firmware.
    let config = Config::from_json_str(CONFIG).unwrap_or_default();

    // Compose the API router with the compile-time asset bundle.
    let mut router = Router::new();
    router.route(Method::Get, "/api/health", health);
    let service = StaticRouter::new(
        router,
        EmbeddedAssets::new(ASSETS),
        config.static_files.index_files.clone(),
        config.mime_overrides.clone(),
    );

    // Bring up smoltcp. Loopback stands in for a board Ethernet MAC; a real
    // deployment swaps it for the chip's device driver.
    let mut device = Loopback::new(Medium::Ethernet);
    let if_config = IfaceConfig::new(EthernetAddress([0x02, 0, 0, 0, 0, 0x01]).into());
    let mut iface = Interface::new(if_config, &mut device, Instant::from_millis(0));
    iface.update_ip_addrs(|addrs| {
        let _ = addrs.push(IpCidr::new(IpAddress::v4(192, 168, 69, 1), 24));
    });

    let mut sockets = SocketSet::new(Vec::new());
    let rx = tcp::SocketBuffer::new(vec![0; 1500]);
    let tx = tcp::SocketBuffer::new(vec![0; 1500]);
    let handle = sockets.add(tcp::Socket::new(rx, tx));
    // This placeholder drives time from a monotonic tick, not a wall clock, so
    // it is clockless for HTTP purposes and must not emit Date (RFC 9110 6.6.1).
    // A real board with an RTC would use `ConnState::new()`.
    let mut server = [(handle, ConnState::with_clock(false))];

    // A monotonic tick advances smoltcp; a real board would source wall-clock
    // time from its RTC and report it as the Unix seconds below.
    let mut tick: i64 = 0;
    serve_smoltcp(
        &mut iface,
        &mut device,
        &mut sockets,
        &mut server,
        config.server.port,
        &service,
        || {
            tick = tick.wrapping_add(1);
            (Instant::from_millis(tick), tick as u64)
        },
    )
}
