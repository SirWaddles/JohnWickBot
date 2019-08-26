extern crate serenity;
extern crate hyper;
extern crate hyper_tls;
extern crate futures;
extern crate tokio;
extern crate serde;
extern crate serde_json;

mod client;
mod db;
mod signal;
mod discord;

use std::sync::Arc;
use std::{cmp, thread, time};
use tokio::timer;
use serde_json::{json, Value as JsonValue};
use hyper::http::Request;
use hyper::{StatusCode, HeaderMap};
use futures::{Async, Stream, try_ready};
use futures::future::{Shared, Future};
use futures::sync::mpsc;
use client::RequestState;

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
struct BroadcastResult {
    status: BroadcastResultType,
    rate_limit_stamp: u64,
    rate_limit_left: u32,
    rate_limit_retry: u64,
    rate_limit_bucket: Option<String>,
    message_body: Option<String>,
}

impl BroadcastResult {
    fn new(status_code: StatusCode, headers: &HeaderMap) -> Self {
        Self {
            status: match status_code {
                StatusCode::OK => BroadcastResultType::Success,
                StatusCode::FORBIDDEN => BroadcastResultType::Forbidden,
                StatusCode::TOO_MANY_REQUESTS => BroadcastResultType::RateLimited,
                StatusCode::NOT_FOUND => BroadcastResultType::NotFound,
                _ => BroadcastResultType::Unknown,
            },
            rate_limit_left: match headers.get("X-RateLimit-Remaining") {
                Some(val) => val.to_str().unwrap().parse().unwrap(),
                None => 0,
            },
            rate_limit_stamp: match headers.get("X-RateLimit-Reset") {
                Some(val) => val.to_str().unwrap().parse().unwrap(),
                None => 0,
            },
            rate_limit_retry: 0,
            rate_limit_bucket: match headers.get("X-RateLimit-Bucket") {
                Some(val) => Some(val.to_str().unwrap().to_owned()),
                None => None,
            },
            message_body: None,
        }
    }

    fn from(&self, message_body: String) -> Self {
        let response: JsonValue = match serde_json::from_str(&message_body) {
            Ok(val) => val,
            Err(_) => return self.clone(),
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
                let limit = response["retry_after"].as_u64().unwrap();
                res.rate_limit_retry = limit;
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

        res.message_body = Some(message_body);

        res
    }
}

fn send_message(client: &Arc<HyperClient>, bot_token: &str, message_content: &str, channel_id: i64) -> Box<dyn Future<Item = BroadcastResult, Error = ()> + Send> {
    let uri = "https://discordapp.com/api/v6/channels/".to_owned() + &channel_id.to_string() + "/messages";
    let request_body = json!({
        "content": message_content
    }).to_string();
    let request = Request::builder()
        .uri(uri)
        .method("POST")
        .header("Authorization", "Bot ".to_owned() + bot_token)
        .header("Content-Type", "application/json")
        .header("User-Agent", "JohnWickBot(https://johnwickbot.shop, 0.1)")
        .body(hyper::Body::from(request_body))
        .unwrap();

    Box::new(client.request(request)
        .and_then(|res| {
            let status = BroadcastResult::new(res.status(), res.headers());
            res.into_body().concat2().map(move |body| (body, status))
        })
        .and_then(|(body, status)| {
            let s = std::str::from_utf8(&body).unwrap();
            Ok(BroadcastResult::from(&status, s.to_owned()))
        })
        .map_err(|err| {
            println!("Error sending response: {}", err);
        }))
}

struct BroadcastInstance {
    attempt: Box<dyn Future<Item = BroadcastResult, Error = ()> + Send>,
    channel_id: i64,
}

impl BroadcastInstance {
    fn new(client: &Arc<HyperClient>, bot_token: &str, message_content: &str, channel_id: i64) -> Self {
        Self {
            attempt: send_message(client, bot_token, message_content, channel_id),
            channel_id: channel_id,
        }
    }

    fn retry(&mut self, client: &Arc<HyperClient>, bot_token: &str, message_content: &str) {
        self.attempt = send_message(client, bot_token, message_content, self.channel_id);
    } 
}

const REQUEST_COUNT: usize = 30;

struct MessageBroadcast {
    client: Arc<HyperClient>,
    total_requests: Vec<BroadcastInstance>,
    ongoing_requests: Vec<BroadcastInstance>,
    bot_token: String,
    message_content: String,
    timer: Option<timer::Delay>,
    db: db::DBConnection,
}

impl MessageBroadcast {
    fn new(client: &Arc<HyperClient>, bot_token: &str, message_content: &str) -> Self {
        let db = db::DBConnection::new().unwrap();
        let channels = db.get_channels().unwrap();

        println!("Starting Broadcast: {}", message_content);
        
        Self {
            client: client.clone(),
            total_requests: channels.into_iter().map(|v| BroadcastInstance::new(client, bot_token, message_content, v)).collect(),
            bot_token: bot_token.to_owned(),
            message_content: message_content.to_owned(),
            ongoing_requests: Vec::new(),
            timer: None,
            db,
        }
    }

    /*fn start_waiting_stamp(&mut self, timestamp: u64) {
        if let Some(_) = self.timer {
            // Already waiting. Don't replace timer.
            return;
        }
        let now_stamp = time::SystemTime::now().duration_since(time::UNIX_EPOCH).unwrap().as_secs();
        let time_until = (timestamp as i64) - (now_stamp as i64);
        if time_until <= 0 {
            // Okay so we've already passed the timestamp. Just move on.
            return;
        }
        let mut timer = timer::Delay::new(time::Instant::now() + time::Duration::from_secs(time_until as u64));
        // Have to poll the timer to register an interest.
        match timer.poll() {
            Ok(_) => (),
            Err(_) => panic!("Timer poll errored"),
        };
        self.timer = Some(timer);
    }*/

    fn start_waiting_retry(&mut self, time_until: u64) {
        if let Some(_) = self.timer {
            // Already waiting. Don't replace timer.
            return;
        }

        let mut timer = timer::Delay::new(time::Instant::now() + time::Duration::from_millis(time_until + 200));
        // Have to poll the timer to register an interest.
        match timer.poll() {
            Ok(_) => (),
            Err(_) => panic!("Timer poll errored"),
        };
        self.timer = Some(timer);
    }

    fn unsubscribe_instance(&self, instance: &BroadcastInstance) {
        self.db.delete_channel(instance.channel_id).unwrap();
    }
}

impl Future for MessageBroadcast {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        // Find out if waiting on rate limits
        let waiting = match &mut self.timer {
            Some(val) => {
                match val.poll() {
                    Ok(Async::Ready(_)) => {
                        self.timer = None;
                        false
                    },
                    Ok(Async::NotReady) => true,
                    Err(_) => panic!("Timer returned error"),
                }
            },
            None => false,
        };

        // Add more requests to concurrent queue
        if waiting == false {
            let new_requests = cmp::min(self.total_requests.len(), REQUEST_COUNT - self.ongoing_requests.len());
            for _ in 0..new_requests {
                self.ongoing_requests.push(self.total_requests.remove(0));
            }
        }

        // Process ongoing requests
        let mut i = 0;
        while i != self.ongoing_requests.len() {
            match self.ongoing_requests[i].attempt.poll() {
                Ok(Async::Ready(res)) => {
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
                                self.start_waiting_retry(res.rate_limit_retry);
                            } else {
                                // No retry value, not sure why.
                                // Just wait 1s
                                self.start_waiting_retry(1000);
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
                Ok(Async::NotReady) => {
                    i += 1;
                },
                Err(_) => {
                    self.ongoing_requests.remove(i);
                }
            };
        }
        if self.ongoing_requests.len() <= 0 && self.total_requests.len() <= 0 {
            println!("Broadcast Finished");
            return Ok(Async::Ready(()));
        }
        Ok(Async::NotReady)
    }
}

struct HyperRuntime {
    client: Arc<HyperClient>,
    bot_token: String,
    receiver: mpsc::UnboundedReceiver<RequestState>,
    exit_status: Shared<signal::TerminationFuture>,
}

impl Future for HyperRuntime {
    type Item = ();

    type Error = ();

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        match self.exit_status.poll() {
            Ok(Async::Ready(_)) => return Ok(Async::Ready(())),
            Ok(Async::NotReady) => (),
            Err(_) => return Err(()),
        };
        loop {
            let request = match try_ready!(self.receiver.poll()) {
                Some(val) => val,
                None => return Ok(Async::Ready(())),
            };


            match request {
                RequestState::ImageBroadcast(filename) => {
                    let message = "https://johnwickbot.shop/".to_owned() + &filename;
                    tokio::spawn(MessageBroadcast::new(&self.client, &self.bot_token, &message));
                },
                RequestState::MessageBroadcast(message) => {
                    tokio::spawn(MessageBroadcast::new(&self.client, &self.bot_token, &message));
                },
                _ => (),
            };
        }
    }
}

fn main() {
    let https = hyper_tls::HttpsConnector::new(4).unwrap();
    let http_client = hyper::Client::builder().build::<_, hyper::Body>(https);

    let unb_channel = mpsc::unbounded::<RequestState>();

    let term_future = signal::TerminationFuture::new().shared();

    let bot_token = std::env::var("DISCORD_TOKEN").unwrap();
    let hyper_runtime = HyperRuntime {
        client: Arc::new(http_client),
        bot_token,
        receiver: unb_channel.1,
        exit_status: term_future.clone(),
    };

    let client_runtime = client::ClientFuture::new(unb_channel.0, term_future.clone()).map_err(|_| println!("Client Error"));
    let mut discord_client = discord::build_client();

    let discord_end = discord::DiscordTermination::new(discord_client.shard_manager.clone(), term_future.clone());

    let runtime_thread = thread::spawn(move || {
        tokio::run(client_runtime.join3(hyper_runtime, discord_end).and_then(|_| {
            Ok(())
        }));
    });

    discord_client.start_autosharded().unwrap();

    runtime_thread.join().unwrap();
}
