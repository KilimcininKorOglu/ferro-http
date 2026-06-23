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
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use mio::net::{TcpListener, TcpStream};
use mio::{Events, Interest, Poll, Token};

use ferro_core::conn::{Connection, Step};
use ferro_core::service::Service;

const LISTENER: Token = Token(0);
const READ_CHUNK: usize = 16 * 1024;

/// Runtime tuning derived from configuration.
pub struct Options {
    /// How long an idle connection may live before being closed.
    pub idle_timeout: Duration,
    /// Maximum number of simultaneous connections.
    pub max_connections: usize,
}

struct Conn {
    socket: TcpStream,
    token: Token,
    state: Connection,
    out: Vec<u8>,
    wants_write: bool,
    close_after_flush: bool,
    last_activity: Instant,
}

impl Conn {
    fn new(socket: TcpStream, token: Token) -> Conn {
        Conn {
            socket,
            token,
            state: Connection::new(),
            out: Vec::new(),
            wants_write: false,
            close_after_flush: false,
            last_activity: Instant::now(),
        }
    }
}

/// Runs the event loop, serving requests through `service`. Never returns
/// except on a fatal poll error.
pub fn serve<S: Service>(addr: SocketAddr, service: &S, options: &Options) -> io::Result<()> {
    let mut poll = Poll::new()?;
    let mut events = Events::with_capacity(1024);
    let mut listener = TcpListener::bind(addr)?;
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;

    let mut conns: HashMap<Token, Conn> = HashMap::new();
    let mut next_token = 1usize;
    let tick = Duration::from_secs(1);

    loop {
        poll.poll(&mut events, Some(tick))?;

        for event in events.iter() {
            match event.token() {
                LISTENER => accept_all(
                    &mut poll,
                    &listener,
                    &mut conns,
                    &mut next_token,
                    options.max_connections,
                ),
                token => {
                    let drop_conn = match conns.get_mut(&token) {
                        Some(conn) => {
                            conn.last_activity = Instant::now();
                            handle_ready(&poll, conn, event.is_readable(), service)
                        }
                        None => false,
                    };
                    if drop_conn {
                        close_conn(&poll, &mut conns, token);
                    }
                }
            }
        }

        sweep_idle(&poll, &mut conns, options.idle_timeout);
    }
}

/// Accepts every pending connection until the listener would block.
fn accept_all(
    poll: &mut Poll,
    listener: &TcpListener,
    conns: &mut HashMap<Token, Conn>,
    next_token: &mut usize,
    max_connections: usize,
) {
    loop {
        match listener.accept() {
            Ok((mut socket, _addr)) => {
                if conns.len() >= max_connections {
                    // Over capacity: drop the socket, closing it.
                    continue;
                }
                let token = Token(*next_token);
                *next_token += 1;
                if poll
                    .registry()
                    .register(&mut socket, token, Interest::READABLE)
                    .is_ok()
                {
                    conns.insert(token, Conn::new(socket, token));
                }
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
        let mut buf = [0u8; READ_CHUNK];
        loop {
            match conn.socket.read(&mut buf) {
                Ok(0) => return true, // peer closed
                Ok(n) => conn.state.feed(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => return true,
            }
        }

        // Drive the state machine over everything buffered (handles pipelining).
        loop {
            match conn.state.step(service) {
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
    }

    if flush(conn) {
        return true;
    }
    update_interest(poll, conn)
}

/// Writes pending bytes until the socket would block or the buffer drains.
/// Returns true on a fatal write error.
fn flush(conn: &mut Conn) -> bool {
    while !conn.out.is_empty() {
        match conn.socket.write(&conn.out) {
            Ok(0) => return true,
            Ok(n) => {
                conn.out.drain(..n);
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return true,
        }
    }
    false
}

/// Reregisters interest after I/O, and reports whether the connection is done.
fn update_interest(poll: &Poll, conn: &mut Conn) -> bool {
    if conn.out.is_empty() {
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
