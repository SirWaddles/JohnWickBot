use std::str;
use std::time::{Duration, Instant};
use futures::{Future, Async};
use futures::sync::mpsc;
use tokio::net::{TcpStream, tcp::ConnectFuture};
use tokio::timer::{Delay, Error as TimerError};
use tokio::prelude::AsyncRead;

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

pub struct RequestState {
    pub channel_id: u64,
    pub message_content: String,
}

enum ClientFutureState {
    Connecting(ConnectFuture),
    Reading(TcpStream),
    Waiting(Delay),
    Disconnected,
}

pub struct ClientFuture {
    state: ClientFutureState,
    sender: mpsc::UnboundedSender<RequestState>,
}

impl ClientFuture {
    pub fn new(sender: mpsc::UnboundedSender<RequestState>) -> Self {
        Self {
            state: ClientFutureState::Connecting(Self::connect()),
            sender,
        }
    }

    fn connect() -> ConnectFuture {
        TcpStream::connect(&"127.0.0.1:27020".parse().unwrap())
    }

    fn process_data(&self, data: Vec<u8>, len: usize) {
        match str::from_utf8(&data[0..len]) {
            Ok(v) => {
                println!("Data: {}", v);
                self.sender.unbounded_send(RequestState {
                    message_content: "Hi there!".to_owned(),
                    channel_id: 613583885699383307,
                }).unwrap();
            },
            Err(_) => return,
        }
    }
}

impl Future for ClientFuture {
    type Item = ();
    type Error = ClientError;

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        loop {
            match &mut self.state {
                ClientFutureState::Disconnected => {
                    println!("disconnected");
                    let when = Instant::now() + Duration::from_secs(30);
                    let mut delay = Delay::new(when);
                    delay.poll()?;
                    self.state = ClientFutureState::Waiting(delay);
                    break
                },
                ClientFutureState::Waiting(delay) => {
                    println!("waiting");
                    match delay.poll() {
                        Ok(Async::Ready(_)) => {
                            let mut connect = Self::connect();
                            connect.poll()?;
                            self.state = ClientFutureState::Connecting(connect);
                            break
                        },
                        Ok(Async::NotReady) => break,
                        Err(_) => {
                            return Err(ClientError);
                        }
                    }
                },
                ClientFutureState::Connecting(connect) => {
                    println!("connecting");
                    match connect.poll() {
                        Ok(Async::Ready(stream)) => {
                            self.state = ClientFutureState::Reading(stream);
                        },
                        Ok(Async::NotReady) => break,
                        Err(_) => {
                            self.state = ClientFutureState::Disconnected;
                        }
                    }
                },
                ClientFutureState::Reading(stream) => {
                    println!("reading");
                    let mut data = vec![0u8; 256];
                    match stream.poll_read(&mut data) {
                        Ok(Async::Ready(bytes)) => {
                            self.process_data(data, bytes);
                        },
                        Ok(Async::NotReady) => break,
                        Err(_) => {
                            self.state = ClientFutureState::Disconnected;
                        },
                    };
                },
            };
        }
        Ok(Async::NotReady)
    }
}