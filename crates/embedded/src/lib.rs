//! `ferro-embedded` — `no_std` smoltcp transport for the ferro web server.
//!
//! This crate is the embedded counterpart of the std profile's mio event loop.
//! It drives the same transport-agnostic [`Connection`] state machine from
//! `ferro-core`, but over smoltcp TCP sockets instead of OS sockets, so the same
//! protocol logic serves requests on bare metal with no operating system.
//!
//! [`pump_connection`] is the unit of work: given one already-listening smoltcp
//! socket and its [`ConnState`], it ingests received bytes, advances the state
//! machine, flushes the response, and re-arms the socket for the next client.
//! [`serve_smoltcp`] is the thin forever-loop that polls the interface and pumps
//! every server socket. The pump is exercised on the host over a loopback device
//! (see the tests), which validates the protocol path without hardware.
//!
//! The crate is `no_std` for real builds but links the standard test harness
//! under `cfg(test)`, so the loopback transport test runs with `cargo test`.
#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;

use ferro_core::conn::{Connection, Step};
use ferro_core::service::Service;

use smoltcp::iface::{Interface, SocketHandle, SocketSet};
use smoltcp::phy::Device;
use smoltcp::socket::tcp;
use smoltcp::time::Instant;

/// A transport-level failure on a smoltcp TCP socket.
///
/// These wrap smoltcp's per-operation errors; the server treats any of them as a
/// reason to drop and re-arm the offending connection, never as fatal.
#[derive(Debug)]
pub enum TransportError {
    /// Arming a socket to listen failed.
    Listen(tcp::ListenError),
    /// Receiving buffered bytes failed.
    Recv(tcp::RecvError),
    /// Enqueuing response bytes failed.
    Send(tcp::SendError),
}

impl From<tcp::ListenError> for TransportError {
    fn from(err: tcp::ListenError) -> TransportError {
        TransportError::Listen(err)
    }
}

impl From<tcp::RecvError> for TransportError {
    fn from(err: tcp::RecvError) -> TransportError {
        TransportError::Recv(err)
    }
}

impl From<tcp::SendError> for TransportError {
    fn from(err: tcp::SendError) -> TransportError {
        TransportError::Send(err)
    }
}

/// Per-socket state carried across poll iterations.
///
/// Holds the core [`Connection`] (the request buffer and policy), the bytes of a
/// response still waiting for send-window space, and whether the connection must
/// close once that response is flushed.
pub struct ConnState {
    conn: Connection,
    out: Vec<u8>,
    close: bool,
}

impl ConnState {
    /// Creates fresh per-socket state with the default connection policy.
    pub fn new() -> ConnState {
        ConnState {
            conn: Connection::new(),
            out: Vec::new(),
            close: false,
        }
    }

    /// Clears all state for reuse by the next client on the same socket.
    fn reset(&mut self) {
        self.conn = Connection::new();
        self.out.clear();
        self.close = false;
    }
}

impl Default for ConnState {
    fn default() -> ConnState {
        ConnState::new()
    }
}

/// Advances one server socket by a single poll's worth of work.
///
/// The socket must already be part of a polled [`SocketSet`]. A fully-closed
/// socket is re-armed to listen on `listen_port` with fresh state; otherwise any
/// received bytes are fed to the connection, every complete request is
/// dispatched through `service`, and the resulting bytes are flushed as the send
/// window allows. When a response asks to close, the TCP close begins once those
/// bytes are fully flushed.
///
/// `now_unix` is the wall-clock time in seconds since the Unix epoch, used for
/// the HTTP `Date` header.
///
/// # Errors
///
/// Returns [`TransportError`] if a smoltcp listen/recv/send operation fails.
pub fn pump_connection<S: Service>(
    socket: &mut tcp::Socket<'_>,
    state: &mut ConnState,
    listen_port: u16,
    service: &S,
    now_unix: u64,
) -> Result<(), TransportError> {
    // A fully-closed socket is re-armed for the next client with fresh state.
    if !socket.is_open() {
        socket.listen(listen_port)?;
        state.reset();
        return Ok(());
    }

    // Pull any received bytes into the connection buffer.
    if socket.can_recv() {
        socket.recv(|buf| {
            state.conn.feed(buf);
            (buf.len(), ())
        })?;
    }

    // Advance the state machine for every complete request now buffered.
    loop {
        match state.conn.step(service, now_unix) {
            Step::NeedMore => break,
            Step::Write { bytes, close } => {
                state.out.extend_from_slice(&bytes);
                if close {
                    // A close response ends the connection; ignore any pipelined
                    // bytes that follow it.
                    state.close = true;
                    break;
                }
            }
        }
    }

    // Flush as much of the pending response as the send window allows.
    if !state.out.is_empty() && socket.can_send() {
        let sent = socket.send_slice(&state.out)?;
        state.out.drain(..sent);
    }

    // Once the closing response is fully flushed, start the TCP close.
    if state.close && state.out.is_empty() {
        socket.close();
        state.close = false;
    }

    Ok(())
}

/// Runs the smoltcp poll loop forever, pumping every server socket.
///
/// `server` pairs each listening socket's handle with its [`ConnState`]. `clock`
/// yields the current `(smoltcp Instant, unix seconds)` on each iteration; the
/// embedded binary supplies it from the board's timer. This busy-polls; a real
/// deployment would sleep on `Interface::poll_delay` between iterations.
pub fn serve_smoltcp<D, S, C>(
    iface: &mut Interface,
    device: &mut D,
    sockets: &mut SocketSet<'_>,
    server: &mut [(SocketHandle, ConnState)],
    listen_port: u16,
    service: &S,
    mut clock: C,
) -> !
where
    D: Device,
    S: Service,
    C: FnMut() -> (Instant, u64),
{
    loop {
        let (instant, now_unix) = clock();
        iface.poll(instant, device, sockets);
        for (handle, state) in server.iter_mut() {
            let socket = sockets.get_mut::<tcp::Socket>(*handle);
            // A per-socket error must not take down the whole server: abort the
            // connection and let the next pass re-arm the socket.
            if pump_connection(socket, state, listen_port, service, now_unix).is_err() {
                socket.abort();
                state.reset();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use ferro_core::http::method::Method;
    use ferro_core::http::request::Request;
    use ferro_core::http::response::Response;
    use ferro_core::http::status::StatusCode;
    use ferro_core::router::{Params, Router};

    use smoltcp::iface::{Config, Interface, SocketSet};
    use smoltcp::phy::{Loopback, Medium};
    use smoltcp::socket::tcp;
    use smoltcp::time::Instant;
    use smoltcp::wire::{EthernetAddress, IpAddress, IpCidr};

    const PORT: u16 = 80;

    fn host() -> IpAddress {
        IpAddress::v4(192, 168, 69, 1)
    }

    fn ok_handler(_req: &Request, _p: &Params) -> Response {
        Response::text(StatusCode::OK, "ok")
    }

    fn router() -> Router {
        let mut r = Router::new();
        r.route(Method::Get, "/", ok_handler);
        r
    }

    fn make_socket() -> tcp::Socket<'static> {
        let rx = tcp::SocketBuffer::new(vec![0; 1500]);
        let tx = tcp::SocketBuffer::new(vec![0; 1500]);
        tcp::Socket::new(rx, tx)
    }

    /// Drives a full TCP handshake and a `GET /` over a loopback device, with the
    /// server side handled exclusively by `pump_connection`. This is the proof
    /// that the core protocol path serves correctly over a real smoltcp socket.
    #[test]
    fn serves_a_get_over_loopback() {
        let mut device = Loopback::new(Medium::Ethernet);
        let config = Config::new(EthernetAddress([0x02, 0, 0, 0, 0, 0x01]).into());
        let mut iface = Interface::new(config, &mut device, Instant::from_millis(0));
        iface.update_ip_addrs(|addrs| {
            addrs
                .push(IpCidr::new(host(), 24))
                .expect("ip address capacity");
        });

        let mut sockets = SocketSet::new(vec![]);
        let server_handle = sockets.add(make_socket());
        let client_handle = sockets.add(make_socket());

        // Arm the server and open the client connection before polling begins.
        sockets
            .get_mut::<tcp::Socket>(server_handle)
            .listen(PORT)
            .expect("listen");
        sockets
            .get_mut::<tcp::Socket>(client_handle)
            .connect(iface.context(), (host(), PORT), 49152)
            .expect("connect");

        let service = router();
        let mut state = ConnState::new();
        let mut request_sent = false;
        let mut response: Vec<u8> = Vec::new();
        let mut done = false;

        for tick in 1..=100u64 {
            iface.poll(Instant::from_millis(tick as i64), &mut device, &mut sockets);

            pump_connection(
                sockets.get_mut::<tcp::Socket>(server_handle),
                &mut state,
                PORT,
                &service,
                tick,
            )
            .expect("server pump");

            let client = sockets.get_mut::<tcp::Socket>(client_handle);
            if client.can_send() && !request_sent {
                client
                    .send_slice(b"GET / HTTP/1.1\r\nConnection: close\r\n\r\n")
                    .expect("client send");
                request_sent = true;
            }
            if client.can_recv() {
                client
                    .recv(|buf| {
                        response.extend_from_slice(buf);
                        (buf.len(), ())
                    })
                    .expect("client recv");
            }
            // The server sent `Connection: close`, so once the client has the
            // response and the receive half is closed, the exchange is complete.
            if request_sent && !response.is_empty() && !client.may_recv() {
                done = true;
                break;
            }
        }

        assert!(
            done,
            "loopback exchange did not complete within the poll budget"
        );
        let text = String::from_utf8(response).expect("utf-8 response");
        assert!(
            text.starts_with("HTTP/1.1 200 OK\r\n"),
            "unexpected status line: {text:?}"
        );
        assert!(
            text.contains("Connection: close\r\n"),
            "missing close header"
        );
        assert!(text.ends_with("ok"), "missing body: {text:?}");
    }
}
