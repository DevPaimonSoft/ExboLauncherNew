use anyhow::Result;
use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{TcpListener, TcpStream},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};
pub struct Ipc {
    pub port: u16,
    pub shutdown: Arc<AtomicBool>,
}
impl Ipc {
    pub fn start(sender: Sender<String>) -> Result<Self> {
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let shutdown = Arc::new(AtomicBool::new(false));
        let stop = shutdown.clone();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                if let Ok((mut socket, peer)) = listener.accept() {
                    if !peer.ip().is_loopback() {
                        continue;
                    }
                    let _ = socket.set_read_timeout(Some(Duration::from_secs(2)));
                    let mut line = String::new();
                    if BufReader::new((&mut socket).take(8193))
                        .read_line(&mut line)
                        .is_ok()
                        && line.len() <= 8192
                        && line.ends_with('\n')
                    {
                        let _ = sender.send(line.trim_end_matches(['\r', '\n']).into());
                    }
                } else {
                    std::thread::sleep(Duration::from_millis(80));
                }
            }
        });
        Ok(Self { port, shutdown })
    }
}
impl Drop for Ipc {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}
pub fn forward(port: u16, link: &str) -> Result<()> {
    crate::protocol::callback_url(link)?;
    let mut socket = TcpStream::connect_timeout(
        &format!("127.0.0.1:{port}").parse()?,
        Duration::from_millis(500),
    )?;
    let data = link.as_bytes();
    socket.write_all(data)?;
    socket.write_all(b"\n")?;
    Ok(())
}
