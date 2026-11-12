// SPDX-License-Identifier: AGPL-3.0-only
use std::io::{BufRead, BufReader};
use std::net::TcpStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[allow(dead_code)]
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A unique path for a test file or directory. The atomic sequence keeps two
/// parallel tests in the same process from generating the same timestamp.
#[allow(dead_code)]
pub fn temp_path(prefix: &str, suffix: &str) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "{prefix}-{}-{nanos}-{sequence}{suffix}",
        std::process::id()
    ))
}

pub struct Server {
    child: Child,
    pub port: u16,
    logs: Arc<Mutex<Vec<String>>>,
}

impl Server {
    pub fn start() -> Self {
        Self::start_with(&[])
    }

    /// Start a server on an ephemeral port and read the bound port back from
    /// the startup line. Binding `--port 0` lets the operating system pick the
    /// port and keeps the listener open, so two tests can never race for the
    /// same port and kill each other's server.
    pub fn start_with(extra_args: &[&str]) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_reru"))
            .args(["--port", "0"])
            .args(extra_args)
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("failed to spawn reru");

        let stderr = child.stderr.take().expect("server stderr is piped");
        let (sender, receiver) = mpsc::channel();
        let logs = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&logs);
        thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                if let Some(address) = line.strip_prefix("ReRu listening on ") {
                    if let Some(port) = address.rsplit(':').next().and_then(|p| p.parse().ok()) {
                        let _ = sender.send(port);
                    }
                }
                sink.lock().unwrap().push(line);
            }
        });

        let port = match receiver.recv_timeout(Duration::from_secs(10)) {
            Ok(port) => port,
            Err(_) => {
                let logs = logs.lock().unwrap().join("\n");
                panic!("server did not report a listening port.\n--- server stderr ---\n{logs}");
            }
        };
        let server = Self { child, port, logs };
        server.wait_until_ready();
        server
    }

    fn wait_until_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            if self.connect().is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        let logs = self.logs.lock().unwrap().join("\n");
        panic!(
            "server reported port {} but did not accept connections.\n--- server stderr ---\n{logs}",
            self.port
        );
    }

    pub fn connect(&self) -> std::io::Result<TcpStream> {
        let stream = TcpStream::connect(("127.0.0.1", self.port))?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        Ok(stream)
    }

    /// Startup lines captured from this server, useful for debugging a failure.
    #[allow(dead_code)]
    pub fn logs(&self) -> String {
        self.logs.lock().unwrap().join("\n")
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
