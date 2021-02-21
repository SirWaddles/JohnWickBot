use std::sync::Arc;
use std::pin::Pin;
use std::cmp;
use std::io::Read;
use hyper::http::Request;
use hyper::{StatusCode, HeaderMap};
use hyper::header::HeaderValue;
use bytes::buf::Buf;
use serde_json::{json, Value as JsonValue};
use futures::future::Future;
use futures::task::{Poll, Context};
use tokio::time as ttime;
use crate::BoxedError;
use crate::db;

type HyperClient = hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>;

#[derive(Debug, Clone)]
enum BroadcastResultType {
    Success,
    Forbidden,
    NotFound,
    UnknownChannel,
    MissingPermissions,
    MissingAccess,
    RateLimited,
    Unknown,
}

#[derive(Debug, Clone)]
struct BroadcastResultInner {
    status: BroadcastResultType,
    rate_limit_stamp: u64,
    rate_limit_left: u32,
    rate_limit_retry: u64,
    rate_limit_bucket: Option<String>,
}

type BroadcastResult = Result<BroadcastResultInner, BoxedError>;

fn parse_header(data: Option<&HeaderValue>) -> Result<u64, BoxedError> {
    match data {
        Some(d) => {
            let f: f64 = d.to_str()?.parse()?;
            Ok(f as u64)
        },
        None => Ok(0),
    }
}

fn parse_header_wrap(data: Option<&HeaderValue>) -> u64 {
    match parse_header(data) {
        Ok(d) => d,
        Err(e) => {
            println!("Error: {:#?}", e);
            0
        }
    }
}

impl BroadcastResultInner {
    fn new(status_code: StatusCode, headers: &HeaderMap) -> Self {
        Self {
            status: match status_code {
                StatusCode::OK => BroadcastResultType::Success,
                StatusCode::FORBIDDEN => BroadcastResultType::Forbidden,
                StatusCode::TOO_MANY_REQUESTS => BroadcastResultType::RateLimited,
                StatusCode::NOT_FOUND => BroadcastResultType::NotFound,
                _ => BroadcastResultType::Unknown,
            },
            rate_limit_left: parse_header_wrap(headers.get("X-RateLimit-Remaining")) as u32,
            rate_limit_stamp: parse_header_wrap(headers.get("X-RateLimit-Reset")),
            rate_limit_retry: 400,
            rate_limit_bucket: match headers.get("X-RateLimit-Bucket") {
                Some(val) => Some(val.to_str().unwrap().to_owned()),
                None => None,
            },
        }
    }

    fn from(&self, message_body: String) -> Self {
        let response: JsonValue = match serde_json::from_str(&message_body) {
            Ok(val) => val,
            Err(e) => {
                println!("JSON Error: {} {}", &message_body, e);
                return self.clone();
            },
        };
        let mut res = self.clone();
        match self.status {
            BroadcastResultType::Forbidden => {
                let code = response["code"].as_u64().unwrap();
                res.status = match code {
                    50001 => BroadcastResultType::MissingAccess,
                    50013 => BroadcastResultType::MissingPermissions,
                    _ => BroadcastResultType::Unknown,
                };
            },
            BroadcastResultType::RateLimited => {
                let limit = response["retry_after"].as_f64().unwrap();
                res.rate_limit_retry = (limit * 1000.0) as u64;
            },
            BroadcastResultType::NotFound => {
                if let Some(code) = response["code"].as_u64() {
                    res.status = match code {
                        10003 => BroadcastResultType::UnknownChannel,
                        _ => BroadcastResultType::Unknown,
                    };
                }
            },
            _ => (),
        };

        res
    }
}

async fn send_message(client: Arc<HyperClient>, bot_token: String, message_content: String, channel_id: i64) -> BroadcastResult {
    let uri = "https://discord.com/api/v8/channels/".to_owned() + &channel_id.to_string() + "/messages";
    let request_body = json!({
        "content": message_content
    }).to_string();
    let request = Request::builder()
        .uri(uri)
        .method("POST")
        .header("Authorization", "Bot ".to_owned() + &bot_token)
        .header("Content-Type", "application/json")
        .header("User-Agent", "JohnWickBot(https://wickshopbot.com, 0.1)")
        .body(hyper::Body::from(request_body))
        .unwrap();

    let req = client.request(request).await.unwrap();
    let status = BroadcastResultInner::new(req.status(), req.headers());
    let body = hyper::body::aggregate(req).await?;

    let mut reader = body.reader();
    let mut data = Vec::new();
    reader.read_to_end(&mut data)?;

    let s = String::from_utf8(data).unwrap();

    Ok(BroadcastResultInner::from(&status, s))

}

struct BroadcastInstance {
    attempt: Pin<Box<dyn Future<Output= BroadcastResult> + Send>>,
    channel_id: i64,
}

impl BroadcastInstance {
    fn new(client: &Arc<HyperClient>, bot_token: &str, message_content: &str, channel_id: i64) -> Self {
        Self {
            attempt: Box::pin(send_message(Arc::clone(client), bot_token.to_owned(), message_content.to_owned(), channel_id)),
            channel_id: channel_id,
        }
    }

    fn retry(&mut self, client: &Arc<HyperClient>, bot_token: &str, message_content: &str) {
        self.attempt = Box::pin(send_message(Arc::clone(client), bot_token.to_owned(), message_content.to_owned(), self.channel_id));
    } 
}

const REQUEST_COUNT: usize = 30;

pub struct MessageBroadcast {
    client: Arc<HyperClient>,
    total_requests: Vec<BroadcastInstance>,
    ongoing_requests: Vec<BroadcastInstance>,
    bot_token: String,
    message_content: String,
    timer: Option<Pin<Box<ttime::Sleep>>>,
    db: Arc<db::DBManager>,
}

impl MessageBroadcast {
    pub fn new(db: Arc<db::DBManager>, channels: Vec<i64>, client: Arc<HyperClient>, bot_token: String, message_content: &str) -> Self {
        println!("Starting Broadcast: {}", message_content);
        
        Self {
            client: Arc::clone(&client),
            total_requests: channels.into_iter().map(|v| BroadcastInstance::new(&client, &bot_token, message_content, v)).collect(),
            bot_token: bot_token,
            message_content: message_content.to_owned(),
            ongoing_requests: Vec::new(),
            timer: None,
            db,
        }
    }

    fn start_waiting_retry(&mut self, cx: &mut Context, time_until: u64) {
        if let Some(_) = self.timer {
            // Already waiting. Don't replace timer.
            return;
        }

        let mut timer = Box::pin(ttime::sleep(ttime::Duration::from_millis(time_until + 200)));
        // Have to poll the timer to register an interest.
        match timer.as_mut().poll(cx) {
            Poll::Ready(_) => (),
            Poll::Pending => (),
        };
        self.timer = Some(timer);
    }

    fn unsubscribe_instance(&self, instance: &BroadcastInstance) {
        let db = Arc::clone(&self.db);
        let channel_id = instance.channel_id;
        tokio::spawn(async move {
            db.delete_channel(channel_id).await.unwrap();
        });
    }
}

impl Future for MessageBroadcast {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Self::Output> {
        // Find out if waiting on rate limits
        let waiting = match &mut self.timer {
            Some(val) => {
                match val.as_mut().poll(cx) {
                    Poll::Ready(_) => {
                        self.timer = None;
                        false
                    },
                    Poll::Pending => true,
                }
            },
            None => false,
        };

        // Add more requests to concurrent queue
        if waiting == false {
            let new_requests = cmp::min(self.total_requests.len(), REQUEST_COUNT - self.ongoing_requests.len());
            for _ in 0..new_requests {
                let req = self.total_requests.remove(0);
                self.ongoing_requests.push(req);
            }
        }

        // Process ongoing requests
        let mut i = 0;
        while i != self.ongoing_requests.len() {
            match self.ongoing_requests[i].attempt.as_mut().poll(cx) {
                Poll::Ready(Ok(res)) => {
                    let mut request = self.ongoing_requests.remove(i);
                    match res.status {
                        BroadcastResultType::Success => {
                            // Message delivered, instance removed from queue.
                            // Do nothing here.
                        },
                        BroadcastResultType::MissingAccess | BroadcastResultType::MissingPermissions | BroadcastResultType::UnknownChannel => {
                            // Bot's been removed from channel/guild
                            // Unsubscribe this channel
                            self.unsubscribe_instance(&request);
                        },
                        BroadcastResultType::RateLimited => {
                            // Message didn't get delivered due to rate limits
                            // Likely returned while other requests were processing.
                            // Start it again and move it to the end of the queue
                            request.retry(&self.client, &self.bot_token, &self.message_content);
                            self.total_requests.push(request);
                            // Start wait timer if not already started
                            if res.rate_limit_retry != 0 {
                                self.start_waiting_retry(cx, res.rate_limit_retry);
                            } else {
                                // No retry value, not sure why.
                                // Just wait 1s
                                self.start_waiting_retry(cx, 1000);
                            }
                            // Just monitoring the rate limits for awhile, seeing how many repeats
                            println!("Request Rate Limited: Wait Until {}", res.rate_limit_retry);
                        },
                        BroadcastResultType::Forbidden | BroadcastResultType::NotFound | BroadcastResultType::Unknown => {
                            // An unknown error, just log and move on.
                            println!("Request Error: {:#?}", res);
                        },
                    };
                },
                Poll::Pending => {
                    i += 1;
                },
                Poll::Ready(Err(_)) => {
                    self.ongoing_requests.remove(i);
                }
            };
        }
        if self.ongoing_requests.len() <= 0 && self.total_requests.len() <= 0 {
            println!("Broadcast Finished");
            return Poll::Ready(());
        }
        Poll::Pending
    }
}