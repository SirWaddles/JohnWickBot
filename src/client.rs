use std::str;
use std::time::{Duration, Instant};
use futures::{Future, Async};
use futures::future::Shared;
use futures::sync::mpsc;
use tokio::net::{TcpStream, tcp::ConnectFuture};
use tokio::timer::{Delay, Error as TimerError};
use tokio::prelude::AsyncRead;
use serde::{Deserialize};
use serde_json::Value;
use crate::signal;

#[derive(Deserialize)]
struct InnerMessage {
    #[serde(rename(deserialize="type"))]
    msg_type: String,
    data: Value,
}

#[derive(Deserialize)]
struct MessageType {
    #[serde(rename(deserialize="type"))]
    msg_type: String,
    data: InnerMessage,
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

impl From<serde_json::Error> for ClientError {
    fn from(err: serde_json::Error) -> Self {
        // yes
        println!("error: {}", err);
        Self
    }
}

pub enum RequestState {
    MessageBroadcast(String),
    ImageBroadcast(String),
    ImageRequest(String),
}

fn parse_message(message: &str) -> Result<RequestState, ClientError> {
    let request: MessageType = serde_json::from_str(message)?;
    if request.msg_type == "app.broadcast" {
        if request.data.msg_type == "image" {
            match request.data.data.as_str() {
                Some(path) => return Ok(RequestState::ImageBroadcast(path.to_owned())),
                None => return Err(ClientError),
            };
        }
        if request.data.msg_type == "message_broadcast" {
            match request.data.data.as_str() {
                Some(msg) => return Ok(RequestState::MessageBroadcast(msg.to_owned())),
                None => return Err(ClientError),
            };
        }
    }
    if request.msg_type == "app.receive_message" {
        match request.data.data.as_str() {
            Some(msg) => return Ok(RequestState::ImageRequest(msg.to_owned())),
            None => return Err(ClientError),
        };
    }
    Err(ClientError)
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
    exit_status: Shared<signal::TerminationFuture>,
}

impl ClientFuture {
    pub fn new(sender: mpsc::UnboundedSender<RequestState>, exit_status: Shared<signal::TerminationFuture>) -> Self {
        Self {
            state: ClientFutureState::Connecting(Self::connect()),
            sender,
            exit_status,
        }
    }

    fn connect() -> ConnectFuture {
        TcpStream::connect(&"127.0.0.1:27020".parse().unwrap())
    }

    fn process_data(&self, data: Vec<u8>, len: usize) {
        match str::from_utf8(&data[0..(len - 1)]) {
            Ok(v) => {
                match parse_message(v) {
                    Ok(message) => self.sender.unbounded_send(message).unwrap(),
                    Err(_) => println!("Could not parse message: {}", v),
                };
            },
            Err(_) => return,
        }
    }
}

impl Future for ClientFuture {
    type Item = ();
    type Error = ClientError;

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        match self.exit_status.poll() {
            Ok(Async::Ready(_)) => return Ok(Async::Ready(())),
            Ok(Async::NotReady) => (),
            Err(_) => return Err(ClientError),
        };
        loop {
            match &mut self.state {
                ClientFutureState::Disconnected => {
                    let when = Instant::now() + Duration::from_secs(30);
                    let delay = Delay::new(when);
                    self.state = ClientFutureState::Waiting(delay);
                },
                ClientFutureState::Waiting(delay) => {
                    match delay.poll() {
                        Ok(Async::Ready(_)) => {
                            let connect = Self::connect();
                            self.state = ClientFutureState::Connecting(connect);
                        },
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Err(_) => {
                            return Err(ClientError);
                        }
                    }
                },
                ClientFutureState::Connecting(connect) => {
                    match connect.poll() {
                        Ok(Async::Ready(stream)) => {
                            self.state = ClientFutureState::Reading(stream);
                        },
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Err(_) => {
                            self.state = ClientFutureState::Disconnected;
                        }
                    }
                },
                ClientFutureState::Reading(stream) => {
                    let mut data = vec![0u8; 256];
                    match stream.poll_read(&mut data) {
                        Ok(Async::Ready(bytes)) => {
                            self.process_data(data, bytes);
                        },
                        Ok(Async::NotReady) => return Ok(Async::NotReady),
                        Err(_) => {
                            self.state = ClientFutureState::Disconnected;
                        },
                    };
                },
            };
        }
    }
}
