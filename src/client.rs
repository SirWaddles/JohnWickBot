use std::str;
use std::time::{Duration, Instant};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use futures::{Future, Async, Poll, Stream};
use futures::future::Shared;
use futures::sync::{mpsc, oneshot};
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

#[derive(Clone, Serialize, Deserialize)]
struct InnerMessage {
    #[serde(rename(serialize="type", deserialize="type"), default="empty_string")]
    msg_type: String,
    #[serde(default="empty_value")]
    request_id: Option<u32>,
    data: Value,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MessageRequest {
    #[serde(rename(serialize="type", deserialize="type"))]
    msg_type: String,
    data: InnerMessage,
}

impl MessageRequest {
    pub fn new(msg_type: &str, data: String, request_id: u32) -> Self {
        Self {
            msg_type: "app.send_message".to_owned(),
            data: InnerMessage {
                msg_type: msg_type.to_owned(),
                request_id: Some(request_id),
                data: Value::String(data),
            },
        }
    }

    pub fn as_str(&self) -> String {
        serde_json::to_string(self).unwrap() + "\x0c"
    }

    pub fn get_data(&self) -> &Value {
        &self.data.data
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
    Other,
}

pub struct ResponseFuture {
    receiver: oneshot::Receiver<MessageRequest>,
    timeout: Delay,
}

impl Future for ResponseFuture {
    type Item = MessageRequest;
    type Error = ClientError;

    fn poll(&mut self) -> Poll<Self::Item, Self::Error> {
        match self.receiver.poll() {
            Ok(Async::Ready(msg)) => Ok(Async::Ready(msg)),
            Ok(Async::NotReady) => {
                match self.timeout.poll() {
                    Ok(Async::Ready(_)) => Err(ClientError),
                    Ok(Async::NotReady) => Ok(Async::NotReady),
                    Err(_) => Err(ClientError),
                }
            },
            Err(_) => Err(ClientError),
        }
    }
}

pub struct MessageManager {
    request_id: u32,
    active_messages: HashMap<u32, oneshot::Sender<MessageRequest>>,
    sender: mpsc::UnboundedSender<MessageRequest>,
}

impl MessageManager {
    pub fn new(sender: mpsc::UnboundedSender<MessageRequest>) -> Self {
        Self {
            request_id: 0,
            active_messages: HashMap::new(),
            sender,
        }
    }

    pub fn add_message(&mut self, msg_type: &str, msg: String) -> ResponseFuture {
        self.request_id += 1;
        let message = MessageRequest::new(msg_type, msg, self.request_id);
        self.sender.unbounded_send(message).unwrap();
        let (msg_send, msg_recv) = oneshot::channel::<MessageRequest>();
        self.active_messages.insert(self.request_id, msg_send);

        let future = ResponseFuture {
            receiver: msg_recv,
            timeout: Delay::new(Instant::now() + Duration::from_secs(20)),
        };

        future
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
    messages: Vec<MessageRequest>,
    message_manager: Arc<Mutex<MessageManager>>,
    sender: mpsc::UnboundedSender<RequestState>,
    receiver: mpsc::UnboundedReceiver<MessageRequest>,
    exit_status: Shared<signal::TerminationFuture>,
}

impl ClientFuture {
    pub fn new(sender: mpsc::UnboundedSender<RequestState>, receiver: mpsc::UnboundedReceiver<MessageRequest>, exit_status: Shared<signal::TerminationFuture>, message_manager: Arc<Mutex<MessageManager>>) -> Self {
        Self {
            state: ClientFutureState::Connecting(Self::connect()),
            sender,
            exit_status,
            messages: Vec::new(),
            receiver,
            message_manager,
        }
    }

    fn connect() -> ConnectFuture {
        TcpStream::connect(&"127.0.0.1:27020".parse().unwrap())
    }

    fn process_data(&self, data: Vec<u8>, len: usize) {
        println!("Received Message {} bytes long.", len);
        match str::from_utf8(&data[0..(len - 1)]) {
            Ok(v) => {
                match self.parse_message(v) {
                    Ok(message) => self.sender.unbounded_send(message).unwrap(),
                    Err(_) => println!("Could not parse message: {}", v),
                };
            },
            Err(_) => return,
        }
    }

    fn parse_message(&self, message: &str) -> Result<RequestState, ClientError> {
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
            if let Some(request_id) = request.data.request_id {
                let mut manager = self.message_manager.lock().unwrap();
                if manager.active_messages.contains_key(&request_id) {
                    let response = manager.active_messages.remove(&request_id).unwrap();
                    if let Err(_) = response.send(request.clone()) {
                        println!("Error sending response to queue");
                    }
                }
            }
            return Ok(RequestState::Other);
        }
        Err(ClientError)
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
