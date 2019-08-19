extern crate mio;

use std::io::Read;
use std::str;
use std::{thread, time};
use mio::net::TcpStream;
use mio::{Events, Poll, PollOpt, Token, Ready};

fn process_data(data: Vec<u8>) {
    match str::from_utf8(&data) {
        Ok(v) => println!("data: {} end", v),
        Err(_) => return,
    }
}

fn client_poll() -> std::io::Result<()> {
    let mut stream = TcpStream::connect(&"127.0.0.1:27020".parse().expect("idk"))?;
    stream.set_keepalive(Some(time::Duration::from_secs(30)))?;
    let poll = Poll::new()?;
    let mut events = Events::with_capacity(256);
    poll.register(&stream, Token(0), Ready::readable() | Ready::writable(), PollOpt::edge())?;

    loop {
        poll.poll(&mut events, Some(time::Duration::from_millis(500)))?;

        for event in &events {
            if event.readiness().is_readable() {
                let mut buf: Vec<u8> = Vec::new();
                match stream.read_to_end(&mut buf) {
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        true
                    },
                    Err(e) => return Err(e),
                    Ok(_) => false,
                };
                process_data(buf);
            }
        }
    }
}

pub fn client_retry() -> std::io::Result<()> {
    loop {
        match client_poll() {
            Ok(_) => return Ok(()),
            Err(ref e) if e.kind() == std::io::ErrorKind::ConnectionReset || e.kind() == std::io::ErrorKind::ConnectionRefused => {
                println!("Connection reset. Attempting reconnect in 30 seconds.");
                thread::sleep(time::Duration::from_secs(30));
                continue
            },
            Err(e) => return Err(e),
        };
    }
}