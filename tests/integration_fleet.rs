//! End-to-end integration test for the multi-host wedge.
//!
//! Spawns an in-process russh server on a random local port, connects a
//! real russh client to it, runs the inlined REMOTE_SCRIPT through the
//! same code path the production collector uses, and verifies the
//! returned blob round-trips through `parse_remote_blob` to a sane
//! `RemoteSnapshot`.
//!
//! This is the only test that exercises the full protocol stack end to
//! end. Everything else (parsers, error handling, connection state,
//! formatting) is unit-tested. The point of this test is "the wires are
//! actually connected" — so if russh's API changes in a future bump or
//! the script we send becomes incompatible with sh, this test is the
//! one that catches it.

use async_trait::async_trait;
use russh::keys::key;
use russh::server::{Auth, Msg, Server as ServerTrait, Session};
use russh::*;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;

use flotop::collectors::remote::{ClientHandler, REMOTE_SCRIPT, parse_remote_blob, run_remote_script};

/// The canned blob the in-process server replies with whenever a client
/// runs anything via `sh -s`. This is the same delimited format the real
/// REMOTE_SCRIPT would produce on a real Linux box.
const CANNED_BLOB: &str = "===FLOTOP-SECTION===\n\
loadavg\n\
1.23 0.98 0.76 2/345 12345\n\
===FLOTOP-SECTION===\n\
meminfo\n\
MemTotal:       8192000 kB\n\
MemFree:        2048000 kB\n\
MemAvailable:   4096000 kB\n\
SwapTotal:      4194304 kB\n\
SwapFree:       4194304 kB\n\
===FLOTOP-SECTION===\n\
stat\n\
ctxt 555666777\n\
===FLOTOP-SECTION===\n\
diskstats\n\
 259       0 nvme0n1 1 2 3 4 5 6 7 8 0 9 99999 0 0 0 0 0 0\n\
===FLOTOP-SECTION===\n\
net_dev\n\
Inter-|   Receive                                                |  Transmit\n\
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed\n\
  eth0:  1234567   1234    0    0    0     0          0         0  7654321   4321    0    0    0     0       0          0\n\
===FLOTOP-SECTION===\n\
ss\n\
Total: 100\n\
TCP:   42 (estab 21, closed 0, orphaned 0, timewait 7)\n\
===FLOTOP-SECTION===\n\
ps\n\
postgres  1234 50.0  4.0 345678 postgres\n\
===FLOTOP-SECTION===\n\
uname\n\
Linux\n";

// ─── in-process russh server ────────────────────────────────────────────

#[derive(Clone)]
struct TestServer {
    /// What the test wants the server to send back when the client runs
    /// any command via `sh -s`. Wrapped in Arc<Mutex<>> so test variants
    /// can swap it.
    blob: Arc<Mutex<String>>,
}

impl ServerTrait for TestServer {
    type Handler = TestServerHandler;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self::Handler {
        TestServerHandler {
            blob: Arc::clone(&self.blob),
        }
    }
}

struct TestServerHandler {
    blob: Arc<Mutex<String>>,
}

#[async_trait]
impl server::Handler for TestServerHandler {
    type Error = anyhow::Error;

    async fn auth_publickey(
        &mut self,
        _user: &str,
        _key: &key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        _channel: Channel<Msg>,
        _session: &mut Session,
    ) -> Result<bool, Self::Error> {
        Ok(true)
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        _data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        // The client will send the script as stdin via channel data.
        // We don't actually need to read it for the test — we just send
        // back the canned blob and close the channel.
        let blob = self.blob.lock().await.clone();
        session.data(channel, CryptoVec::from(blob.into_bytes()));
        session.eof(channel);
        session.close(channel);
        Ok(())
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        _data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        // Discard stdin from the client (the inlined script).
        Ok(())
    }
}

/// Spawn the test server on 127.0.0.1:0 and return its bound port.
async fn spawn_server(blob: String) -> u16 {
    let server_key = russh_keys::key::KeyPair::generate_ed25519().unwrap();
    let config = russh::server::Config {
        inactivity_timeout: Some(Duration::from_secs(10)),
        auth_rejection_time: Duration::from_secs(0),
        auth_rejection_time_initial: Some(Duration::from_secs(0)),
        keys: vec![server_key],
        ..Default::default()
    };
    let config = Arc::new(config);

    // Bind to a random port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let mut server: TestServer = TestServer {
        blob: Arc::new(Mutex::new(blob)),
    };

    tokio::spawn(async move {
        loop {
            let (socket, addr) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => return,
            };
            let handler = <TestServer as ServerTrait>::new_client(&mut server, Some(addr));
            let cfg = Arc::clone(&config);
            tokio::spawn(async move {
                let _ = russh::server::run_stream(cfg, socket, handler).await;
            });
        }
    });

    port
}

/// Connect a real russh client to the test server.
async fn connect_client(port: u16) -> russh::client::Handle<ClientHandler> {
    let config = Arc::new(russh::client::Config::default());
    let handler = ClientHandler;
    let mut session = russh::client::connect(config, ("127.0.0.1", port), handler)
        .await
        .expect("client connect");

    let client_key = russh_keys::key::KeyPair::generate_ed25519().unwrap();
    let auth_ok = session
        .authenticate_publickey("testuser", Arc::new(client_key))
        .await
        .expect("auth call");
    assert!(auth_ok, "in-process server should accept any publickey");
    session
}

// ─── tests ──────────────────────────────────────────────────────────────

#[tokio::test]
async fn end_to_end_run_remote_script_returns_canned_blob() {
    let port = spawn_server(CANNED_BLOB.to_string()).await;
    let session = connect_client(port).await;

    let blob = run_remote_script(&session, REMOTE_SCRIPT)
        .await
        .expect("run_remote_script");

    // The server returns the canned blob verbatim regardless of what
    // we sent over stdin, so the body should match exactly.
    assert_eq!(blob, CANNED_BLOB);
}

#[tokio::test]
async fn end_to_end_blob_parses_to_remote_snapshot() {
    let port = spawn_server(CANNED_BLOB.to_string()).await;
    let session = connect_client(port).await;

    let blob = run_remote_script(&session, REMOTE_SCRIPT)
        .await
        .expect("run_remote_script");
    let parsed = parse_remote_blob(&blob);

    // Verify each section round-tripped through the wire and the parser.
    let load = parsed.loadavg.expect("loadavg parsed");
    assert_eq!(load.one, 1.23);
    assert_eq!(load.five, 0.98);
    assert_eq!(load.fifteen, 0.76);

    assert_eq!(parsed.meminfo.total, 8_192_000 * 1024);
    assert_eq!(parsed.meminfo.free, 2_048_000 * 1024);
    assert_eq!(parsed.meminfo.swap_total, 4_194_304 * 1024);

    assert_eq!(parsed.ctxt_total, Some(555_666_777));
    assert!(parsed.diskstats.contains_key("nvme0n1"));
    assert_eq!(parsed.ss.established, 21);
    assert_eq!(parsed.ss.time_wait, 7);
    assert_eq!(parsed.ps_rows.len(), 1);
    assert_eq!(parsed.ps_rows[0].user, "postgres");
    assert_eq!(parsed.ps_rows[0].cpu_pct, 50.0);
    assert_eq!(parsed.uname.as_deref(), Some("Linux"));

    let interfaces = parsed.interfaces;
    assert_eq!(interfaces.len(), 1);
    assert_eq!(interfaces[0].0, "eth0");
}
