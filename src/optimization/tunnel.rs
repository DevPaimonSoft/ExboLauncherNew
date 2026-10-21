mod measure;
mod stats;
#[cfg(test)]
mod tests;
use anyhow::{Context, Result, ensure};
pub use measure::{Progress, measure};
use serde::{Deserialize, Serialize};
pub use stats::{Cache, Measured, Sample, best_index, rank, sort};
use std::{
    collections::HashSet,
    net::{SocketAddr, ToSocketAddrs, UdpSocket},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub const PROBE_COUNT: usize = 10;
pub const MIN_REPLIES: usize = 5;
const CONCURRENCY: usize = 8;
const PROBE_INTERVAL: Duration = Duration::from_millis(100);
const PROBE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Default)]
pub struct Control(Arc<AtomicBool>);
impl Control {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    pub fn cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
    fn check(&self) -> Result<()> {
        ensure!(
            !self.cancelled(),
            "Измерение отменено / Measurement cancelled"
        );
        Ok(())
    }
    fn pause(&self, duration: Duration) -> Result<()> {
        let until = Instant::now() + duration;
        while Instant::now() < until {
            self.check()?;
            std::thread::sleep(
                until
                    .saturating_duration_since(Instant::now())
                    .min(Duration::from_millis(20)),
            );
        }
        self.check()
    }
}
pub struct Results {
    pub region: String,
    pub entries: Vec<Measured>,
    pub cache: Cache,
}
pub struct Probe {
    pub client: f64,
    pub server: f64,
    pub limited: bool,
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Endpoint {
    pub name: String,
    pub address: String,
}
#[derive(Deserialize)]
pub struct Pool {
    pub name: String,
    pub tunnels: Vec<Endpoint>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub mode: String,
    pub pools: Vec<Pool>,
    #[serde(default)]
    pub client_to_tunnel_rtt_weight: f64,
}
#[derive(Clone, Debug)]
pub struct Target {
    pub pool: String,
    pub endpoint: Endpoint,
}
pub fn parse_address(address: &str) -> Result<(String, u16)> {
    ensure!(
        address == address.trim()
            && address.len() <= 300
            && !address.chars().any(char::is_whitespace),
        "Адрес туннеля должен иметь вид host:port без пробелов"
    );
    let (host, port) = if address.starts_with('[') {
        let end = address.find(']').context("Некорректный IPv6-адрес")?;
        let host = &address[1..end];
        ensure!(
            host.parse::<std::net::Ipv6Addr>().is_ok(),
            "Некорректный IPv6-адрес"
        );
        (host, &address[end + 1..])
    } else {
        let (host, port) = address.rsplit_once(':').context("Укажите порт туннеля")?;
        ensure!(
            !host.is_empty()
                && host.len() <= 253
                && host.split('.').all(|part| !part.is_empty()
                    && part.len() <= 63
                    && !part.starts_with('-')
                    && !part.ends_with('-')
                    && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')),
            "Некорректное имя узла туннеля"
        );
        (host, &address[host.len()..host.len() + 1 + port.len()])
    };
    let port = port
        .strip_prefix(':')
        .context("Укажите порт после двоеточия")?
        .parse::<u16>()
        .context("Некорректный порт туннеля")?;
    ensure!(
        (1..65535).contains(&port),
        "Порт туннеля: от 1 до 65534 (следующий порт используется для измерения)"
    );
    Ok((host.into(), port))
}
pub fn validate_catalog(catalog: &Catalog) -> Result<()> {
    ensure!(
        catalog.mode == "roxy",
        "Сервер использует неподдерживаемый режим туннелей"
    );
    ensure!(
        catalog.client_to_tunnel_rtt_weight.is_finite()
            && (0. ..=1000.).contains(&catalog.client_to_tunnel_rtt_weight),
        "Некорректный вес RTT"
    );
    ensure!(
        !catalog.pools.is_empty() && catalog.pools.len() <= 100,
        "Некорректное количество групп туннелей"
    );
    let mut count = 0;
    let mut names = HashSet::new();
    for pool in &catalog.pools {
        ensure!(
            !pool.name.is_empty()
                && pool.name.len() <= 128
                && !pool.name.chars().any(char::is_control)
                && names.insert(&pool.name),
            "Некорректное имя группы туннелей"
        );
        for endpoint in &pool.tunnels {
            count += 1;
            ensure!(count <= 1000, "Слишком много туннелей");
            ensure!(
                !endpoint.name.is_empty()
                    && endpoint.name.len() <= 128
                    && !endpoint.name.chars().any(char::is_control),
                "Некорректное имя туннеля"
            );
            parse_address(&endpoint.address)?;
        }
    }
    ensure!(count > 0, "Список туннелей пуст");
    Ok(())
}
#[cfg(test)]
pub fn probe(address: &str, timeout: Duration) -> Result<Probe> {
    probe_with_control(address, timeout, &Control::default())
}
fn probe_with_control(address: &str, timeout: Duration, control: &Control) -> Result<Probe> {
    control.check()?;
    let (host, port) = parse_address(address)?;
    let target: SocketAddr = (host.as_str(), port + 1)
        .to_socket_addrs()?
        .next()
        .context("Адрес туннеля не найден")?;
    let socket = UdpSocket::bind(if target.is_ipv6() {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    })?;
    control.check()?;
    socket.connect(target)?;
    socket.set_write_timeout(Some(timeout))?;
    let token = *uuid::Uuid::new_v4().as_bytes();
    let started = Instant::now();
    socket.send(&token)?;
    let mut bytes = [0; 65536];
    loop {
        control.check()?;
        let remaining = timeout.saturating_sub(started.elapsed());
        ensure!(!remaining.is_zero(), "Туннель не ответил вовремя");
        socket.set_read_timeout(Some(remaining.min(Duration::from_millis(50))))?;
        let size = match socket.recv(&mut bytes) {
            Ok(size) => size,
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                ) =>
            {
                continue;
            }
            Err(e) => return Err(e.into()),
        };
        if size != 21 || bytes[..16] != token {
            continue;
        }
        let server = i32::from_be_bytes(bytes[16..20].try_into().unwrap());
        ensure!(server >= 0, "Туннель вернул отрицательный RTT");
        return Ok(Probe {
            client: started.elapsed().as_secs_f64() * 1000.,
            server: server as f64,
            limited: bytes[20] != 0,
        });
    }
}
