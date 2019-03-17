extern crate interledger;
#[macro_use]
extern crate log;

use env_logger;
use futures::Future;
use interledger::cli;
use net2;
use redis;
use std::{
    process,
    thread::sleep,
    time::{Duration, Instant},
};
use tokio::{runtime::Runtime, timer::Delay};

// Test helpers copied from https://github.com/mitsuhiko/redis-rs/blob/master/tests/support/mod.rs
pub struct RedisServer {
    process: process::Child,
    port: u16,
}

fn get_open_port() -> u16 {
    let listener = net2::TcpBuilder::new_v4()
        .unwrap()
        .reuse_address(true)
        .unwrap()
        .bind("127.0.0.1:0")
        .unwrap()
        .listen(1)
        .unwrap();
    listener.local_addr().unwrap().port()
}

impl RedisServer {
    pub fn new() -> RedisServer {
        let mut cmd = process::Command::new("redis-server");
        // Comment these lines out to see Redis log output
        cmd.stdout(process::Stdio::null())
            .stderr(process::Stdio::null());

        // this is technically a race but we can't do better with
        // the tools that redis gives us :(
        let port = get_open_port();
        cmd.arg("--loglevel").arg("verbose");
        cmd.arg("--port")
            .arg(port.to_string())
            .arg("--bind")
            .arg("127.0.0.1");
        let process = cmd.spawn().unwrap();

        debug!("Spawning redis server on port: {}", port);
        let mut server = RedisServer {
            process: process,
            port,
        };
        server.flush_db();

        server
    }

    pub fn wait(&mut self) {
        self.process.wait().unwrap();
    }

    pub fn stop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }

    fn flush_db(&mut self) {
        let client =
            redis::Client::open(format!("redis://127.0.0.1:{}", self.port).as_str()).unwrap();
        let con;

        let millisecond = Duration::from_millis(1);
        loop {
            match client.get_connection() {
                Err(err) => {
                    if err.is_connection_refusal() {
                        sleep(millisecond);
                    } else {
                        panic!("Could not connect: {}", err);
                    }
                }
                Ok(x) => {
                    con = x;
                    break;
                }
            }
        }
        redis::cmd("FLUSHDB").execute(&con);
        debug!("Flushed db");
    }
}

impl Drop for RedisServer {
    fn drop(&mut self) {
        self.stop()
    }
}

fn delay(ms: u64) -> impl Future<Item = (), Error = ()> {
    Delay::new(Instant::now() + Duration::from_millis(ms)).map_err(|err| panic!(err))
}

#[test]
#[cfg(feature = "test-redis")]
fn btp_end_to_end() {
    let _ = env_logger::init();
    let redis_server = RedisServer::new();
    // let redis_port = 6379;
    let redis_port = redis_server.port;
    let redis_uri = format!("redis://127.0.0.1:{}", redis_port);
    let btp_port = get_open_port();
    let http_port = get_open_port();
    let run = delay(200).and_then(move |_| {
        let redis_uri_clone = redis_uri.clone();
        let create_accounts = delay(50).and_then(move |_| {
            cli::insert_account_redis(
                &redis_uri_clone,
                cli::AccountDetails {
                    ilp_address: Vec::from("example.one"),
                    asset_code: "XYZ".to_string(),
                    asset_scale: 9,
                    btp_incoming_authorization: Some("token-one".to_string()),
                    btp_uri: None,
                    http_endpoint: None,
                    http_incoming_authorization: None,
                    http_outgoing_authorization: None,
                    max_packet_amount: u64::max_value(),
                    is_admin: false,
                },
            )
            .and_then(move |_| {
                cli::insert_account_redis(
                    &redis_uri_clone,
                    cli::AccountDetails {
                        ilp_address: Vec::from("example.two"),
                        asset_code: "XYZ".to_string(),
                        asset_scale: 9,
                        btp_incoming_authorization: Some("token-two".to_string()),
                        btp_uri: None,
                        http_endpoint: None,
                        http_incoming_authorization: None,
                        http_outgoing_authorization: None,
                        max_packet_amount: u64::max_value(),
                        is_admin: false,
                    },
                )
            })
        });

        let redis_uri_clone = redis_uri.clone();
        let spawn_connector = move |_| {
            // Note: this needs to be run AFTER the accounts are created because the
            // store does not currently subscribe to notifications of accounts being created
            // or the routing table being updated
            let connector = interledger::cli::run_connector_redis(
                &redis_uri_clone,
                ([127, 0, 0, 1], btp_port).into(),
                ([127, 0, 0, 1], http_port).into(),
            );
            tokio::spawn(connector);
            Ok(())
        };

        let spsp_server_port = get_open_port();
        let spawn_spsp_server = move |_| {
            let spsp_server = cli::run_spsp_server_btp(
                &format!("btp+ws://:token-one@localhost:{}", btp_port),
                ([127, 0, 0, 1], spsp_server_port).into(),
                true,
            );
            tokio::spawn(spsp_server);
            Ok(())
        };

        create_accounts
            .and_then(spawn_connector)
            .and_then(|_| delay(50))
            .and_then(spawn_spsp_server)
            .and_then(|_| delay(50))
            .and_then(move |_| {
                cli::send_spsp_payment_btp(
                    &format!("btp+ws://:token-two@localhost:{}", btp_port),
                    &format!("http://localhost:{}", spsp_server_port),
                    10000,
                    true,
                )
            })
    });
    let mut runtime = Runtime::new().unwrap();
    runtime.block_on(run).unwrap();
}