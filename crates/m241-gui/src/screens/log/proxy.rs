//! The NMEA relay: repaired lines to the screen log and to a loopback TCP server.
//!
//! `m241 nmea proxy` reads a live stream, repairs RMC/ZDA dates and relays the
//! result to stdout and/or a `--bind` socket. The GUI has no stdout, so the
//! screen's log view is that channel; the socket is the same optional
//! `--bind` server, and the same loopback-only default applies — exposing a
//! live position stream on a network interface is a decision the user has to
//! make explicitly.
//!
//! The relay runs on a thread and publishes a snapshot through a mutex. A serial
//! port has no end of stream, so the relay stops when the screen asks it to, when
//! the optional line limit is reached (`--max-lines`), or when a file source
//! reaches EOF, which makes the same code testable without a unit.

use std::collections::VecDeque;
use std::io::{Read as _, Write as _};
use std::net::{SocketAddr, TcpListener, TcpStream, ToSocketAddrs as _};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use m241_core::protocol::Port;
use m241_core::protocol::port::Transport;

use super::nmea::{self, OffsetSpec};

/// Longest unterminated fragment the relay buffers before dropping it.
///
/// The NMEA 0183 limit is 82 characters; the next power of two keeps a harmless
/// overshoot from being treated as noise. A fragment past this is a baud mismatch
/// or a line that never terminated.
const MAX_LINE_BYTES: usize = 128;

/// Repaired lines the screen keeps. A relay can run for hours; the log view is a
/// recent window, not a recording.
const LOG_LINES: usize = 64;

/// How long an idle serial read yields before the loop checks the stop flag.
const IDLE_SLEEP: std::time::Duration = std::time::Duration::from_millis(5);

/// Where the relay reads from.
#[derive(Debug, Clone)]
pub enum ProxySource {
    /// A live NMEA stream on a serial port.
    Port {
        /// Port name, resolved before the thread starts.
        name: String,
        /// Baud rate of the stream.
        baud: u32,
    },
    /// A captured NMEA file, replayed to the end.
    File(PathBuf),
}

/// One relay run's inputs.
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    /// The stream source.
    pub source: ProxySource,
    /// How to correct the dates.
    pub offset: OffsetSpec,
    /// Optional `host:port` for the TCP server.
    pub bind: Option<String>,
    /// Allow a non-loopback bind.
    pub allow_non_loopback: bool,
    /// Stop after relaying this many lines (`nmea proxy --max-lines`); `None`
    /// runs until stopped or the source ends.
    pub max_lines: Option<usize>,
}

/// What the screen reads from a running relay.
#[derive(Debug, Clone, Default)]
pub struct ProxySnapshot {
    /// Whether the relay thread is still reading.
    pub running: bool,
    /// The bound socket, when one was requested.
    pub bound: Option<String>,
    /// Connected TCP clients.
    pub clients: usize,
    /// Lines relayed so far.
    pub lines: usize,
    /// Why the relay ended, once it has.
    pub ended: Option<String>,
    /// The most recent repaired lines, oldest first.
    pub log: Vec<String>,
}

/// Shared between the thread and the screen. The screen only ever clones a
/// snapshot, so a slow frame cannot hold the relay up.
#[derive(Debug, Default)]
struct ProxyState {
    running: bool,
    bound: Option<String>,
    clients: usize,
    lines: usize,
    ended: Option<String>,
    log: VecDeque<String>,
}

/// A running (or finished) relay.
#[derive(Debug)]
pub struct ProxyHandle {
    shared: Arc<Mutex<ProxyState>>,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl ProxyHandle {
    /// Bind the listener, then start the relay thread.
    ///
    /// The socket is bound here, on the UI thread, so a bad address or a port
    /// already in use is a refusal the screen shows immediately rather than a
    /// failure that only appears in the status line a moment later.
    ///
    /// # Errors
    ///
    /// An invalid address, a bind failure, or a non-loopback address without
    /// `allow_non_loopback`. Opening the source happens on the thread; its
    /// refusal arrives through [`ProxySnapshot::ended`].
    pub fn start(config: ProxyConfig) -> Result<Self, String> {
        let listener = match config.bind.as_deref() {
            Some(spec) => Some(bind_checked(spec, config.allow_non_loopback)?),
            None => None,
        };
        let bound = listener
            .as_ref()
            .and_then(|listener| listener.local_addr().ok())
            .map(|addr| format!("tcp://{addr}"));
        let shared = Arc::new(Mutex::new(ProxyState {
            running: true,
            bound,
            ..ProxyState::default()
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let shared = Arc::clone(&shared);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || relay(config, listener, shared, stop))
        };
        Ok(Self {
            shared,
            stop,
            thread: Some(thread),
        })
    }

    /// The current state, copied so no lock outlives the call.
    #[must_use]
    pub fn snapshot(&self) -> ProxySnapshot {
        let state = lock(&self.shared);
        ProxySnapshot {
            running: state.running,
            bound: state.bound.clone(),
            clients: state.clients,
            lines: state.lines,
            ended: state.ended.clone(),
            log: state.log.iter().cloned().collect(),
        }
    }

    /// Whether the relay thread is still reading.
    #[must_use]
    pub fn is_running(&self) -> bool {
        lock(&self.shared).running
    }

    /// Ask the relay to stop and wait for the thread.
    ///
    /// The wait is bounded by the serial read timeout (200 ms) because the loop
    /// checks the flag between reads. Keeping the handle afterwards lets the
    /// screen show the final counts and log window.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let mut state = lock(&self.shared);
        state.running = false;
        state.clients = 0;
        if state.ended.is_none() {
            state.ended = Some(String::from("停止しました"));
        }
    }
}

impl Drop for ProxyHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Lock the shared state, recovering from a panicked thread.
fn lock(shared: &Arc<Mutex<ProxyState>>) -> MutexGuard<'_, ProxyState> {
    shared
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Bind `spec`, refusing a non-loopback address unless `allow`.
fn bind_checked(spec: &str, allow: bool) -> Result<TcpListener, String> {
    let addresses: Vec<SocketAddr> = spec
        .to_socket_addrs()
        .map_err(|err| format!("待受アドレス {spec:?} を解釈できません: {err}"))?
        .collect();
    if addresses.is_empty() {
        return Err(format!(
            "待受アドレス {spec:?} はソケットアドレスになりません"
        ));
    }
    if !allow && let Some(non_loopback) = addresses.iter().find(|addr| !addr.ip().is_loopback()) {
        return Err(format!(
            "待受をループバック以外の {non_loopback} に開こうとしています。\
             ネットワークに公開する場合は「ループバック以外を許可」を入れてください"
        ));
    }
    let listener = TcpListener::bind(addresses.as_slice())
        .map_err(|err| format!("{spec} で待ち受けできません: {err}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|err| format!("{spec} を非ブロッキングにできません: {err}"))?;
    Ok(listener)
}

/// The relay thread's body.
fn relay(
    config: ProxyConfig,
    listener: Option<TcpListener>,
    shared: Arc<Mutex<ProxyState>>,
    stop: Arc<AtomicBool>,
) {
    let mut source = match Source::open(&config.source) {
        Ok(source) => source,
        Err(message) => {
            let mut state = lock(&shared);
            state.running = false;
            state.ended = Some(message);
            return;
        }
    };
    let mut clients: Vec<TcpStream> = Vec::new();
    let mut pending: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 256];
    let mut relayed = 0usize;

    let ending: String = 'relay: loop {
        if stop.load(Ordering::Relaxed) {
            break String::from("停止しました");
        }
        if let Some(listener) = &listener {
            accept_pending(listener, &mut clients);
        }
        match source.read(&mut chunk) {
            Ok(0) => {
                if source.is_file() {
                    break String::from("入力の終わりに達しました");
                }
                std::thread::sleep(IDLE_SLEEP);
                continue;
            }
            Ok(count) => {
                if let Some(received) = chunk.get(..count) {
                    pending.extend_from_slice(received);
                }
            }
            Err(message) => {
                break format!("中継を止めました: {message}");
            }
        }
        while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
            let raw: Vec<u8> = pending.drain(..=position).collect();
            let text = String::from_utf8_lossy(&raw);
            let (repaired, _changed) = nmea::repair_line(&text, config.offset);
            {
                let mut state = lock(&shared);
                state.lines += 1;
                if state.log.len() >= LOG_LINES {
                    let _dropped = state.log.pop_front();
                }
                state
                    .log
                    .push_back(repaired.trim_end_matches(['\r', '\n']).to_owned());
                state.clients = clients.len();
            }
            relayed += 1;
            broadcast(&mut clients, repaired.as_bytes());
            // `--max-lines` ends the relay from inside the line loop: the limit
            // is a property of the relayed stream, not of a read chunk, so it
            // cannot be checked between reads without overshooting.
            if let Some(max) = config.max_lines
                && relayed >= max
            {
                break 'relay format!("指定の {max} 行に達しました");
            }
        }
        if pending.len() >= MAX_LINE_BYTES {
            pending.clear();
        }
    };

    let mut state = lock(&shared);
    state.running = false;
    state.clients = 0;
    if state.ended.is_none() {
        state.ended = Some(ending);
    }
}

/// The byte source, with the idle-versus-EOF distinction the loop needs.
#[derive(Debug)]
enum Source {
    Serial(Port),
    File(std::fs::File),
}

impl Source {
    fn open(config: &ProxySource) -> Result<Self, String> {
        match config {
            ProxySource::Port { name, baud } => Port::open(name, *baud)
                .map(Self::Serial)
                .map_err(|err| format!("{name} を開けません: {err}")),
            ProxySource::File(path) => std::fs::File::open(path)
                .map(Self::File)
                .map_err(|err| format!("{} を開けません: {err}", path.display())),
        }
    }

    fn read(&mut self, buffer: &mut [u8]) -> Result<usize, String> {
        match self {
            Self::Serial(port) => port
                .read(buffer)
                .map_err(|err| format!("ポートを読めません: {err}")),
            Self::File(file) => file
                .read(buffer)
                .map_err(|err| format!("入力を読めません: {err}")),
        }
    }

    fn is_file(&self) -> bool {
        matches!(self, Self::File(_))
    }
}

/// Accept every queued TCP client without blocking the stream.
fn accept_pending(listener: &TcpListener, clients: &mut Vec<TcpStream>) {
    loop {
        match listener.accept() {
            Ok((stream, _peer)) => {
                let _ = stream.set_nonblocking(false);
                clients.push(stream);
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => break,
        }
    }
}

/// Write `bytes` to every client, dropping the ones that disconnected.
fn broadcast(clients: &mut Vec<TcpStream>, bytes: &[u8]) {
    clients.retain_mut(|stream| stream.write_all(bytes).is_ok() && stream.flush().is_ok());
}

#[cfg(test)]
mod tests {
    use super::super::test_support::ScratchDir;
    use super::*;
    use std::time::{Duration, Instant};

    fn scratch_with(name: &str, text: &str) -> (ScratchDir, PathBuf) {
        let scratch = ScratchDir::new("proxy");
        let path = scratch.join(name);
        std::fs::write(&path, text).expect("write the capture");
        (scratch, path)
    }

    fn wait_until_finished(handle: &ProxyHandle) -> ProxySnapshot {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let snapshot = handle.snapshot();
            if !snapshot.running {
                return snapshot;
            }
            assert!(Instant::now() < deadline, "relay did not finish in time");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_file_source_relays_the_repaired_line_and_reports_the_end() {
        let (_scratch, path) = scratch_with(
            "capture.nmea",
            "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,170906,,,N*7F\r\n",
        );
        let handle = ProxyHandle::start(ProxyConfig {
            source: ProxySource::File(path),
            offset: OffsetSpec::Auto,
            bind: None,
            allow_non_loopback: false,
            max_lines: None,
        })
        .expect("start the relay");
        let snapshot = wait_until_finished(&handle);
        assert_eq!(snapshot.lines, 1);
        assert_eq!(
            snapshot.log.first().map(String::as_str),
            Some("$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,030526,,,N*74")
        );
        assert_eq!(snapshot.ended.as_deref(), Some("入力の終わりに達しました"));
    }

    #[test]
    fn the_relay_passes_a_gga_line_through_unchanged() {
        let gga = "$GPGGA,022127.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00";
        let (_scratch, path) = scratch_with(
            "capture.nmea",
            &format!(
                "$GPRMC,022127.542,V,8960.0000,N,00000.0000,E,0.00,0.00,170906,,,N*7F\r\n{gga}\r\n"
            ),
        );
        let handle = ProxyHandle::start(ProxyConfig {
            source: ProxySource::File(path),
            offset: OffsetSpec::Auto,
            bind: None,
            allow_non_loopback: false,
            max_lines: None,
        })
        .expect("start the relay");
        let snapshot = wait_until_finished(&handle);
        assert_eq!(snapshot.lines, 2);
        assert_eq!(snapshot.log.get(1).map(String::as_str), Some(gga));
    }

    #[test]
    fn a_line_limit_ends_the_relay_after_that_many_lines() {
        // `nmea proxy --max-lines` is the CLI's termination condition; the GUI
        // has the stop button, and the limit is the same stated end.
        let (_scratch, path) = scratch_with(
            "capture.nmea",
            "$GPGGA,022127.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00\r\n\
             $GPGGA,022128.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00\r\n\
             $GPGGA,022129.54,3540.87416,N,13946.02750,E,1,08,0.9,80.0,M,0.0,M,,*00\r\n",
        );
        let handle = ProxyHandle::start(ProxyConfig {
            source: ProxySource::File(path),
            offset: OffsetSpec::Auto,
            bind: None,
            allow_non_loopback: false,
            max_lines: Some(2),
        })
        .expect("start the relay");
        let snapshot = wait_until_finished(&handle);
        assert_eq!(snapshot.lines, 2, "the relay stopped at the limit");
        assert_eq!(snapshot.ended.as_deref(), Some("指定の 2 行に達しました"));
    }

    #[test]
    fn a_non_loopback_bind_is_refused_without_the_flag() {
        let error = ProxyHandle::start(ProxyConfig {
            source: ProxySource::File(PathBuf::from("unused.nmea")),
            offset: OffsetSpec::Auto,
            bind: Some(String::from("0.0.0.0:0")),
            allow_non_loopback: false,
            max_lines: None,
        })
        .expect_err("the default bind is loopback only");
        assert!(error.contains("ループバック以外"), "got {error}");
    }

    #[test]
    fn a_loopback_bind_reports_its_socket_and_stops_on_request() {
        let (scratch, path) = scratch_with("empty.nmea", "");
        let mut handle = ProxyHandle::start(ProxyConfig {
            source: ProxySource::File(path),
            offset: OffsetSpec::Auto,
            bind: Some(String::from("127.0.0.1:0")),
            allow_non_loopback: false,
            max_lines: None,
        })
        .expect("start the relay");
        let snapshot = handle.snapshot();
        assert!(
            snapshot
                .bound
                .as_deref()
                .is_some_and(|bound| bound.starts_with("tcp://127.0.0.1:")),
            "the bound socket is reported: {snapshot:?}"
        );
        let _wait = wait_until_finished(&handle);
        handle.stop();
        assert!(!handle.is_running());
        assert!(!scratch.path().as_os_str().is_empty());
    }
}
