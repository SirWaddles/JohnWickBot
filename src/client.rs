use std::str;
use std::time::{Duration, Instant};
use futures::{Future, Async, Poll, Stream};
use futures::future::Shared;
use futures::sync::mpsc;
use tokio::net::{TcpStream, tcp::ConnectFuture};
use tokio::timer::{Delay, Error as TimerError};
use tokio::prelude::{AsyncRead, AsyncWrite};
use serde::{Serialize, Deserialize};
use serde_json::Value;
use crate::signal;

fn empty_value() -> Option<u32> {
    None
}

fn empty_string() -> String {
    "".to_owned()
}

#[derive(Serialize, Deserialize)]
struct InnerMessage {
    #[serde(rename(serialize="type", deserialize="type"), default="empty_string")]
    msg_type: String,
    #[serde(default="empty_value")]
    request_id: Option<u32>,
    data: Value,
}

#[derive(Serialize, Deserialize)]
pub struct MessageRequest {
    #[serde(rename(serialize="type", deserialize="type"))]
    msg_type: String,
    data: InnerMessage,
}

impl MessageRequest {
    pub fn new(msg_type: &str, data: String) -> Self {
        Self {
            msg_type: "app.send_message".to_owned(),
            data: InnerMessage {
                msg_type: msg_type.to_owned(),
                request_id: Some(1),
                data: Value::String(data),
            },
        }
    }

    pub fn as_str(&self) -> String {
        serde_json::to_string(self).unwrap() + "\x0c"
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
    let request: MessageRequest = serde_json::from_str(message)?;
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
    messages: Vec<MessageRequest>,
    sender: mpsc::UnboundedSender<RequestState>,
    receiver: mpsc::UnboundedReceiver<MessageRequest>,
    exit_status: Shared<signal::TerminationFuture>,
}

impl ClientFuture {
    pub fn new(sender: mpsc::UnboundedSender<RequestState>, receiver: mpsc::UnboundedReceiver<MessageRequest>,exit_status: Shared<signal::TerminationFuture>) -> Self {
        Self {
            state: ClientFutureState::Connecting(Self::connect()),
            sender,
            exit_status,
            messages: Vec::new(),
            receiver,
        }
    }

    fn connect() -> ConnectFuture {
        TcpStream::connect(&"127.0.0.1:27020".parse().unwrap())
    }

    fn process_data(&self, data: Vec<u8>, len: usize) {
        println!("Received Message {} bytes long.", len);
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

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.exit_status.poll() {
            Ok(Async::Ready(_)) => return Ok(Async::Ready(())),
            Ok(Async::NotReady) => (),
            Err(_) => return Err(ClientError),
        };
        // Receive messages to send
        loop {
            match self.receiver.poll() {
                Ok(Async::Ready(Some(message))) => {
                    self.messages.push(message);
                },
                Ok(Async::Ready(None)) => return Ok(Async::Ready(())),
                Ok(Async::NotReady) => break,
                Err(_) => return Err(ClientError),
            }
        }

        // Socket handling (receiving/sending/reconnecting)
        loop {
            match &mut self.state {
                ClientFutureState::Disconnected => {
                    println!("Socket disconnected");
                    let when = Instant::now() + Duration::from_secs(30);
                    let delay = Delay::new(when);
                    self.state = ClientFutureState::Waiting(delay);
                },
                ClientFutureState::Waiting(delay) => {
                    match delay.poll() {
                        Ok(Async::Ready(_)) => {
                            println!("Attempting Reconnect");
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
                    while self.messages.len() > 0 {
                        println!("polling write");
                        match stream.poll_write(&self.messages[0].as_str().as_bytes()) {
                            Ok(Async::Ready(bytes)) => {
                                println!("sent bytes: {}", bytes);
                                self.messages.remove(0);
                            },
                            Ok(Async::NotReady) => break,
                            Err(_) => break,
                        };
                    }

                    let mut data = vec![0u8; 256];
                    match stream.poll_read(&mut data) {
                        Ok(Async::Ready(bytes)) => {
                            // I have no idea why this is happening, but I'm spinning on zero-byte messages on a disconnected socket.
                            if bytes > 0 {
                                self.process_data(data, bytes);
                            } else {
                                self.state = ClientFutureState::Disconnected;
                            }
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
