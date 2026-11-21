// Every test crate compiles this module and uses a different part of it.
#![allow(dead_code)]

use ledger::database;
use sqlx::PgPool;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::time::Duration;
use testcontainers::core::{ContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

const PASSWORD: &str = "ledger";

pub struct Postgres {
    pub url: String,
    _container: ContainerAsync<GenericImage>,
}

pub async fn postgres() -> Postgres {
    let container = GenericImage::new("postgres", "17-alpine")
        .with_exposed_port(ContainerPort::Tcp(5432))
        .with_wait_for(WaitFor::message_on_stderr(
            "database system is ready to accept connections",
        ))
        .with_env_var("POSTGRES_PASSWORD", PASSWORD)
        .start()
        .await
        .expect("the throwaway PostgreSQL starts");
    let host = container
        .get_host()
        .await
        .expect("the container has a host");
    let port = container
        .get_host_port_ipv4(5432)
        .await
        .expect("the container maps the PostgreSQL port");
    let url = format!("postgres://postgres:{PASSWORD}@{host}:{port}/postgres");
    wait_until_connectable(&url).await;
    Postgres {
        url,
        _container: container,
    }
}

async fn wait_until_connectable(url: &str) {
    for _ in 0..50 {
        if database::connect(url).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    panic!("the throwaway PostgreSQL does not accept connections");
}

pub async fn migrated_pool(postgres: &Postgres) -> PgPool {
    let pool = database::connect(&postgres.url).await.expect("connect");
    database::migrate(&pool).await.expect("migrate");
    pool
}

pub fn get_status(address: SocketAddr, path: &str) -> u16 {
    let mut stream = TcpStream::connect(address).expect("connect to the service");
    write!(
        stream,
        "GET {path} HTTP/1.1\r\nHost: ledger\r\nConnection: close\r\n\r\n"
    )
    .expect("send the request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read the response");
    response
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .expect("the response has a status line")
}

pub fn post(address: SocketAddr, path: &str, body: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(address).expect("connect to the service");
    write!(
        stream,
        "POST {path} HTTP/1.1\r\nHost: ledger\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    )
    .expect("send the request");
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .expect("read the response");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|status| status.parse().ok())
        .expect("the response has a status line");
    let body = response
        .split_once("\r\n\r\n")
        .map_or_else(String::new, |(_, body)| body.to_string());
    (status, body)
}
