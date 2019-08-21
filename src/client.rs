use std::str;
use std::time::{Duration, Instant};
use futures::{Future, Async};
use tokio::net::{TcpStream, tcp::ConnectFuture};
use tokio::timer::{Delay, Error as TimerError};
use tokio::prelude::AsyncRead;

fn process_data(data: Vec<u8>, len: usize) {
    match str::from_utf8(&data[0..len]) {
        Ok(v) => println!("data: {} end", v),
        Err(_) => return,
    }
}

pub struct ClientError;

impl From<std::io::Error> for ClientError {
    fn from(_err: std::io::Error) -> Self {
        Self
    }
}

impl From<TimerError> for ClientError {
    fn from(_err: TimerError) -> Self {
        Self
    }
}

enum ClientFutureState {
    Connecting(ConnectFuture),
    Reading(TcpStream),
    Waiting(Delay),
    Disconnected,
}

pub struct ClientFuture {
    state: ClientFutureState,
}

impl ClientFuture {
    pub fn new() -> Self {
        Self {
            state: ClientFutureState::Connecting(Self::connect()),
        }
    }

    fn connect() -> ConnectFuture {
        TcpStream::connect(&"127.0.0.1:27020".parse().unwrap())
    }
}

impl Future for ClientFuture {
    type Item = ();
    type Error = ClientError;

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        match &mut self.state {
            ClientFutureState::Disconnected => {
                let when = Instant::now() + Duration::from_secs(30);
                let mut delay = Delay::new(when);
                delay.poll()?;
                self.state = ClientFutureState::Waiting(delay);
            },
            ClientFutureState::Waiting(delay) => {
                match delay.poll() {
                    Ok(Async::Ready(_)) => {
                        let mut connect = Self::connect();
                        connect.poll()?;
                        self.state = ClientFutureState::Connecting(connect);
                    },
                    Ok(Async::NotReady) => (),
                    Err(_) => {
                        return Err(ClientError);
                    }
                }
            },
            ClientFutureState::Connecting(connect) => {
                match connect.poll() {
                    Ok(Async::Ready(stream)) => {
                        self.state = ClientFutureState::Reading(stream);
                        self.poll()?;
                    },
                    Ok(Async::NotReady) => (),
                    Err(_) => {
                        self.state = ClientFutureState::Disconnected;
                        self.poll()?;
                    }
                }
            },
            ClientFutureState::Reading(stream) => {
                let mut data = vec![0u8; 256];
                match stream.poll_read(&mut data) {
                    Ok(Async::Ready(bytes)) => {
                        process_data(data, bytes);
                        self.poll()?;
                    },
                    Ok(Async::NotReady) => (),
                    Err(_) => {
                        self.state = ClientFutureState::Disconnected;
                        self.poll()?;
                    },
                };
            },
        };
        Ok(Async::NotReady)
    }
}