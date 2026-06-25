//! Non-blocking event-loop transport built on mio (epoll/kqueue/IOCP).
//!
//! mio delivers edge-triggered readiness, so every readable/writable/accept
//! event is drained in a loop until `WouldBlock`; the kernel will not re-notify
//! for bytes already buffered. Each connection is either waiting to read or
//! (when a write could not complete) also waiting to write, and its interest is
//! reregistered on every transition to avoid spurious writable wakeups.
//!
//! All time lives here, never in the core: idle connections are reaped by a
//! deadline sweep on a fixed poll tick.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};

use ferro_core::conn::{Connection, ResponsePolicy, Step};
use ferro_core::service::Service;

const LISTENER: Token = Token(0);
const READ_CHUNK: usize = 16 * 1024;
/// How long, after a shutdown signal, to let in-flight connections finish.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

/// Shared TLS configuration threaded to each reactor. With the `tls` feature it
/// is an optional `rustls::ServerConfig` shared across reactors; without it, a
/// zero-sized placeholder so the transport keeps one signature and code path.
#[cfg(feature = "tls")]
pub type SharedTls = Option<std::sync::Arc<rustls::ServerConfig>>;
#[cfg(not(feature = "tls"))]
pub type SharedTls = ();

/// Runtime tuning derived from configuration.
#[derive(Clone, Copy)]
pub struct Options {
    /// How long an idle connection may live before being closed.
    pub idle_timeout: Duration,
    /// Maximum number of simultaneous connections (per reactor after fan-out).
    pub max_connections: usize,
    /// Whether responses receive the standard security headers.
    pub security_headers: bool,
    /// Maximum accepted request body size, in bytes.
    pub max_body: usize,
    /// Reactor thread count; `0` means "derive from available parallelism".
    pub worker_threads: usize,
}

struct Conn {
    socket: TcpStream,
    token: Token,
    state: Connection,
    out: Vec<u8>,
    wants_write: bool,
    close_after_flush: bool,
    last_activity: Instant,
    /// Per-connection TLS session; `None` for plaintext connections.
    #[cfg(feature = "tls")]
    tls: Option<Box<rustls::ServerConnection>>,
}

impl Conn {
    fn new(
        socket: TcpStream,
        token: Token,
        security_headers: bool,
        max_body: usize,
        peer: [u8; 16],
    ) -> Conn {
        Conn {
            socket,
            token,
            // The std profile always runs on a host with a real clock.
            state: Connection::with_policy(ResponsePolicy {
                security_headers,
                ..ResponsePolicy::default()
            })
            .max_body(max_body)
            .peer(peer),
            out: Vec::new(),
            wants_write: false,
            close_after_flush: false,
            last_activity: Instant::now(),
            #[cfg(feature = "tls")]
            tls: None,
        }
    }

    /// Reads available transport bytes and feeds decrypted plaintext to the core
    /// state machine. Returns true if the peer closed or a fatal error occurred.
    fn read_in(&mut self) -> bool {
        #[cfg(feature = "tls")]
        if let Some(tls) = self.tls.as_mut() {
            return tls_read_in(&mut self.socket, tls, &mut self.state);
        }
        plain_read_in(&mut self.socket, &mut self.state)
    }

    /// Hands queued plaintext output to the transport. For TLS this encrypts it
    /// into the session's send buffer; for plaintext it stays in `out` as-is.
    fn queue_out(&mut self) {
        #[cfg(feature = "tls")]
        if let Some(tls) = self.tls.as_mut() {
            tls_queue_out(tls, &mut self.out);
        }
    }

    /// Writes pending transport bytes to the socket. Returns true on a fatal
    /// write error.
    fn write_out(&mut self) -> bool {
        #[cfg(feature = "tls")]
        if let Some(tls) = self.tls.as_mut() {
            return tls_write_out(&mut self.socket, tls);
        }
        plain_write_out(&mut self.socket, &mut self.out)
    }

    /// Whether the transport still has bytes waiting to be written to the socket.
    fn pending_write(&self) -> bool {
        #[cfg(feature = "tls")]
        if let Some(tls) = self.tls.as_ref() {
            return tls.wants_write();
        }
        !self.out.is_empty()
    }

    /// Begins a clean shutdown. For TLS this queues a `close_notify` alert so the
    /// peer sees an orderly close rather than a truncated stream. Called once,
    /// when the response asks to close.
    fn begin_close(&mut self) {
        #[cfg(feature = "tls")]
        if let Some(tls) = self.tls.as_mut() {
            tls.send_close_notify();
        }
    }
}

/// Maps a peer socket address to a 16-byte key (IPv4 is IPv6-mapped).
fn ip_key(addr: SocketAddr) -> [u8; 16] {
    match addr.ip() {
        IpAddr::V4(v4) => v4.to_ipv6_mapped().octets(),
        IpAddr::V6(v6) => v6.octets(),
    }
}

/// Serves requests through `service`, fanned out across reactor threads.
///
/// On Unix each reactor owns a `SO_REUSEPORT` listener on the same address and
/// the kernel load-balances accepts across them; elsewhere a single reactor is
/// used. `service` is shared by reference across scoped threads, so it must be
/// `Sync`. Never returns except on a fatal error from a reactor.
pub fn serve<S: Service + Sync>(
    addr: SocketAddr,
    service: &S,
    options: &Options,
    tls: &SharedTls,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    let workers = worker_count(options.worker_threads);
    // Each reactor enforces the connection cap independently; divide so the
    // configured total is preserved rather than multiplied by the worker count.
    let per_worker = Options {
        max_connections: (options.max_connections / workers).max(1),
        ..*options
    };

    if workers <= 1 {
        return run_reactor(addr, service, &per_worker, tls, shutdown);
    }

    std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| scope.spawn(move || run_reactor(addr, service, &per_worker, tls, shutdown)))
            .collect();
        for handle in handles {
            match handle.join() {
                Ok(result) => result?,
                Err(_) => return Err(io::Error::other("reactor thread panicked")),
            }
        }
        Ok(())
    })
}

/// Resolves the reactor count: the configured value, or available parallelism
/// when `0`. Always at least 1; forced to 1 without `SO_REUSEPORT`.
fn worker_count(configured: usize) -> usize {
    #[cfg(not(unix))]
    {
        let _ = configured;
        1
    }
    #[cfg(unix)]
    {
        if configured > 0 {
            configured
        } else {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        }
    }
}

/// Builds a non-blocking listener with address/port reuse, ready for mio.
fn build_listener(addr: SocketAddr) -> io::Result<TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::for_address(addr), Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    #[cfg(unix)]
    socket.set_reuse_port(true)?;
    socket.bind(&addr.into())?;
    socket.listen(1024)?;
    let listener: std::net::TcpListener = socket.into();
    listener.set_nonblocking(true)?;
    Ok(TcpListener::from_std(listener))
}

/// Runs one reactor with its own poll, listener, and connection set. Never
/// returns except on a fatal poll error.
fn run_reactor<S: Service>(
    addr: SocketAddr,
    service: &S,
    options: &Options,
    tls: &SharedTls,
    shutdown: &AtomicBool,
) -> io::Result<()> {
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);
    let mut listener = build_listener(addr)?;
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;

    let mut conns: HashMap<Token, Conn> = HashMap::new();
    let mut next_token = 1usize;
    let tick = Duration::from_secs(1);

    // Serve until a shutdown signal arrives (observed within one poll tick).
    while !shutdown.load(Ordering::Relaxed) {
        poll.poll(&mut events, Some(tick))?;
        for event in events.iter() {
            match event.token() {
                LISTENER => accept_all(
                    &mut poll,
                    &listener,
                    &mut conns,
                    &mut next_token,
                    options,
                    tls,
                ),
                token => handle_token(&poll, &mut conns, token, event.is_readable(), service),
            }
        }
        sweep_idle(&poll, &mut conns, options.idle_timeout);
    }

    // Graceful drain: stop accepting and finish in-flight connections up to a
    // grace deadline, then return so the thread can join.
    let _ = poll.registry().deregister(&mut listener);
    let deadline = Instant::now() + SHUTDOWN_GRACE;
    while !conns.is_empty() && Instant::now() < deadline {
        poll.poll(&mut events, Some(tick))?;
        for event in events.iter() {
            let token = event.token();
            if token != LISTENER {
                handle_token(&poll, &mut conns, token, event.is_readable(), service);
            }
        }
        sweep_idle(&poll, &mut conns, options.idle_timeout);
    }
    Ok(())
}

/// Processes a readiness event for one connection token, dropping it on close.
fn handle_token<S: Service>(
    poll: &Poll,
    conns: &mut HashMap<Token, Conn>,
    token: Token,
    readable: bool,
    service: &S,
) {
    let drop_conn = match conns.get_mut(&token) {
        Some(conn) => {
            conn.last_activity = Instant::now();
            handle_ready(poll, conn, readable, service)
        }
        None => false,
    };
    if drop_conn {
        close_conn(poll, conns, token);
    }
}

/// Accepts every pending connection until the listener would block.
#[cfg_attr(not(feature = "tls"), allow(unused_variables))]
fn accept_all(
    poll: &mut Poll,
    listener: &TcpListener,
    conns: &mut HashMap<Token, Conn>,
    next_token: &mut usize,
    options: &Options,
    tls: &SharedTls,
) {
    loop {
        match listener.accept() {
            Ok((mut socket, peer_addr)) => {
                if conns.len() >= options.max_connections {
                    // Over capacity: drop the socket, closing it.
                    continue;
                }
                // Disable Nagle's algorithm: send small responses immediately.
                let _ = socket.set_nodelay(true);
                let token = Token(*next_token);
                *next_token += 1;
                if poll
                    .registry()
                    .register(&mut socket, token, Interest::READABLE)
                    .is_err()
                {
                    continue;
                }
                // `mut` is used only when the tls feature wraps the connection.
                #[cfg_attr(not(feature = "tls"), allow(unused_mut))]
                let mut conn = Conn::new(
                    socket,
                    token,
                    options.security_headers,
                    options.max_body,
                    ip_key(peer_addr),
                );
                #[cfg(feature = "tls")]
                if let Some(config) = tls.as_ref() {
                    match rustls::ServerConnection::new(config.clone()) {
                        Ok(session) => conn.tls = Some(Box::new(session)),
                        Err(_) => {
                            let _ = poll.registry().deregister(&mut conn.socket);
                            continue;
                        }
                    }
                }
                conns.insert(token, conn);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
}

/// Handles a readiness event for one connection. Returns true if it should be
/// dropped (peer closed, error, or a completed close-after-flush).
fn handle_ready<S: Service>(poll: &Poll, conn: &mut Conn, readable: bool, service: &S) -> bool {
    if readable && !conn.close_after_flush {
        if conn.read_in() {
            return true; // peer closed or a fatal transport error
        }

        // Drive the state machine over everything buffered (handles pipelining).
        let now = unix_secs();
        loop {
            match conn.state.step(service, now) {
                Step::NeedMore => break,
                Step::Write { bytes, close } => {
                    conn.out.extend_from_slice(&bytes);
                    if close {
                        conn.close_after_flush = true;
                        break;
                    }
                }
            }
        }
        // Encrypt the response first, then queue close_notify after it: rustls
        // drops application data written once close_notify has been sent.
        conn.queue_out();
        if conn.close_after_flush {
            conn.begin_close();
        }
    }

    if conn.write_out() {
        return true;
    }
    update_interest(poll, conn)
}

/// Plaintext read: pulls socket bytes straight into the core state machine.
/// Returns true if the peer closed or a fatal error occurred.
fn plain_read_in(socket: &mut TcpStream, state: &mut Connection) -> bool {
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match socket.read(&mut buf) {
            Ok(0) => return true, // peer closed
            Ok(n) => state.feed(&buf[..n]),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return true,
        }
    }
    false
}

/// Plaintext write: writes `out` straight to the socket until it would block or
/// drains. Returns true on a fatal write error.
fn plain_write_out(socket: &mut TcpStream, out: &mut Vec<u8>) -> bool {
    while !out.is_empty() {
        match socket.write(out) {
            Ok(0) => return true,
            Ok(n) => {
                out.drain(..n);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return true,
        }
    }
    false
}

/// TLS read: pumps ciphertext from the socket through rustls (advancing the
/// handshake or decrypting data) and feeds any plaintext to the state machine.
/// Returns true if the peer closed or a TLS error occurred.
#[cfg(feature = "tls")]
fn tls_read_in(
    socket: &mut TcpStream,
    tls: &mut rustls::ServerConnection,
    state: &mut Connection,
) -> bool {
    let mut buf = [0u8; READ_CHUNK];
    loop {
        match tls.read_tls(socket) {
            Ok(0) => return true, // peer closed the TCP connection
            Ok(_) => {
                if tls.process_new_packets().is_err() {
                    return true; // TLS protocol error; drop the connection
                }
                // Drain decrypted plaintext between reads, not only at the end:
                // rustls bounds its internal plaintext buffer and `read_tls`
                // errors once it fills, so a body larger than that buffer must
                // be consumed as it arrives or the connection would be dropped.
                loop {
                    match tls.reader().read(&mut buf) {
                        Ok(0) => break, // nothing more buffered right now
                        Ok(n) => state.feed(&buf[..n]),
                        Err(_) => break, // WouldBlock or transient
                    }
                }
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return true,
        }
    }
    false
}

/// Encrypts queued plaintext into the TLS session's outgoing buffer.
#[cfg(feature = "tls")]
fn tls_queue_out(tls: &mut rustls::ServerConnection, out: &mut Vec<u8>) {
    if !out.is_empty() {
        // rustls' writer buffers all bytes, so a short write cannot occur.
        let _ = tls.writer().write_all(out);
        out.clear();
    }
}

/// TLS write: drains the session's outgoing ciphertext to the socket. Returns
/// true on a fatal write error.
#[cfg(feature = "tls")]
fn tls_write_out(socket: &mut TcpStream, tls: &mut rustls::ServerConnection) -> bool {
    while tls.wants_write() {
        match tls.write_tls(socket) {
            Ok(0) => return true,
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return true,
        }
    }
    false
}

/// Reregisters interest after I/O, and reports whether the connection is done.
fn update_interest(poll: &Poll, conn: &mut Conn) -> bool {
    if !conn.pending_write() {
        if conn.close_after_flush {
            return true; // fully flushed and asked to close
        }
        if conn.wants_write {
            // Drop writable interest to avoid spinning on spurious wakeups.
            let _ = poll
                .registry()
                .reregister(&mut conn.socket, conn.token, Interest::READABLE);
            conn.wants_write = false;
        }
    } else if !conn.wants_write {
        // Pending bytes: also wait for writability.
        let _ = poll.registry().reregister(
            &mut conn.socket,
            conn.token,
            Interest::READABLE | Interest::WRITABLE,
        );
        conn.wants_write = true;
    }
    false
}

fn close_conn(poll: &Poll, conns: &mut HashMap<Token, Conn>, token: Token) {
    if let Some(mut conn) = conns.remove(&token) {
        let _ = poll.registry().deregister(&mut conn.socket);
    }
}

/// Current Unix time in seconds, for the `Date` header.
fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Closes connections idle longer than the timeout.
fn sweep_idle(poll: &Poll, conns: &mut HashMap<Token, Conn>, idle_timeout: Duration) {
    let now = Instant::now();
    let expired: Vec<Token> = conns
        .iter()
        .filter(|(_, c)| now.duration_since(c.last_activity) > idle_timeout)
        .map(|(t, _)| *t)
        .collect();
    for token in expired {
        close_conn(poll, conns, token);
    }
}
