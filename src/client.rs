use std::sync::{Arc, Mutex};
use std::collections::HashMap;
use futures::channel::mpsc::{unbounded, UnboundedReceiver, UnboundedSender};
use futures::channel::oneshot;
use futures::stream::StreamExt;
use futures::future::FutureExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpStream, tcp::OwnedReadHalf};
use tokio::time::{sleep, Duration};
use serde::{Serialize, Deserialize};
use serde_json::Value as JSONValue;
use crate::JWResult;

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
    data: JSONValue,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct MessageRequest {
    #[serde(rename(serialize="type", deserialize="type"))]
    msg_type: String,
    data: InnerMessage,
}

impl MessageRequest {
    fn new(msg_type: &str, data: String, request_id: u32) -> Self {
        Self {
            msg_type: "app.send_message".to_owned(),
            data: InnerMessage {
                msg_type: msg_type.to_owned(),
                request_id: Some(request_id),
                data: JSONValue::String(data),
            },
        }
    }

    fn as_str(&self) -> String {
        serde_json::to_string(self).unwrap() + "\x0c"
    }

    fn as_buf(&self) -> Vec<u8> {
        self.as_str().into_bytes()
    }

    pub fn get_data(&self) -> &JSONValue {
        &self.data.data
    }
}

pub struct ClientManager {
    sender: UnboundedSender<MessageRequest>,
    request_id: u32,
    active_messages: HashMap<u32, oneshot::Sender<JWResult<MessageRequest>>>,
}

pub async fn send_message(manager: &Arc<Mutex<ClientManager>>, msg_type: &str, data: String) -> JWResult<MessageRequest> {
    let mut msg_recv = {
        let mut lock = manager.lock().unwrap();
        lock.request_id += 1;
        let req_id = lock.request_id;
        let msg = MessageRequest::new(msg_type, data, req_id);
        let (msg_send, msg_recv) = oneshot::channel::<JWResult<MessageRequest>>();
        lock.sender.unbounded_send(msg)?;
        lock.active_messages.insert(req_id, msg_send);

        msg_recv
    };

    let mut s = Box::pin(sleep(Duration::from_secs(10))).fuse();

    futures::select! {
        r = msg_recv => {
            r?
        },
        _ = s => {
            Err("Message Failed to Send".into())
        },
    }
}

impl ClientManager {
    fn parse_message(&mut self, msg: &[u8]) -> JWResult<()> {
        let msg = std::str::from_utf8(&msg[0..(msg.len() - 1)])?;
        let req: MessageRequest = serde_json::from_str(msg)?;

        if req.msg_type == "app.receive_message" {
            if let Some(req_id) = req.data.request_id {
                if self.active_messages.contains_key(&req_id) {
                    let response = self.active_messages.remove(&req_id).unwrap();
                    if let Err(_) = response.send(Ok(req)) {
                        println!("Error sending response to queue");
                    }
                } else {
                    println!("Received Message with ID {} with no sender", req_id);
                }
            }
        }

        Ok(())
    }
}

async fn read_loop(manager: Arc<Mutex<ClientManager>>, mut reader: OwnedReadHalf) {
    let mut read_msg: Vec<u8> = Vec::new();
    let mut read_buf = vec![0u8; 256];
    while let Ok(bytes) = reader.read(&mut read_buf).await {
        if bytes <= 0 { return; }
        let slen = read_msg.len();
        read_msg.resize(slen + bytes, 0);
        read_msg[slen..(slen + bytes)].copy_from_slice(&mut read_buf[0..bytes]);
        if read_msg.contains(&12) {
            let mut lock = manager.lock().unwrap();
            match lock.parse_message(&read_msg) {
                Ok(_) => (),
                Err(e) => {
                    println!("Message Parse Failed: {}", e);
                },
            };
            read_msg.clear();
        }
    } 
}

async fn connect_loop(manager: Arc<Mutex<ClientManager>>, client_read: UnboundedReceiver<MessageRequest>) {
    let mut peek_read = Box::pin(client_read.peekable());
    loop {
        sleep(Duration::from_secs(5)).await;
        println!("Connecting to JW Server");
        let stream = match TcpStream::connect("127.0.0.1:27020").await {
            Ok(s) => s,
            Err(e) => {
                println!("Connection Failed - Retrying: {}", e);
                continue;
            },
        };
        let (read_socket, mut write_socket) = stream.into_split();

        let read_manager = Arc::clone(&manager);
        tokio::spawn(async move {
            read_loop(read_manager, read_socket).await;
        });

        // Attempt to write next available item by peeking, then reconnect if write socket unavailable.

        while let Some(msg) = peek_read.as_mut().peek().await {
            let buf = msg.as_buf();
            match write_socket.write_all(&buf).await {
                Ok(_) => {
                    // Consume peeked item.
                    peek_read.next().await;
                },
                Err(e) => {
                    println!("Connection Error: {}", e);
                    break;
                },
            };
        }
    }
}

pub fn connect_client() -> Arc<Mutex<ClientManager>> {
    let (client_write, client_read) = unbounded::<MessageRequest>();
    let manager = Arc::new(Mutex::new(ClientManager {
        sender: client_write,
        request_id: 0,
        active_messages: HashMap::new(),
    }));

    let manager_connect = Arc::clone(&manager);
    tokio::spawn(async move {
        connect_loop(manager_connect, client_read).await;
    });  

    manager
}