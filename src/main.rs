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

use std::thread;
use serde_json::json;
use hyper::http::Request;
use futures::{Async, Stream, try_ready};
use futures::future::{Shared, Future};
use futures::sync::mpsc;
use client::RequestState;

type HyperClient = hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>;

fn send_message(client: &HyperClient, bot_token: &str, message_content: &str, channel_id: i64) -> Box<dyn Future<Item = (), Error = ()> + Send> {
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
            println!("req status: {}", res.status());
            res.into_body().concat2()
        })
        .and_then(|body| {
            let s = std::str::from_utf8(&body).unwrap();
            println!("body: {}", s);
            Ok(())
        })
        .map_err(|err| {
            println!("Error sending response: {}", err);
        }))
}

const REQUEST_COUNT: usize = 100;

struct MessageBroadcast {
    total_requests: Vec<Box<dyn Future<Item = (), Error = ()> + Send>>,
}

impl MessageBroadcast {
    fn new(client: &HyperClient, bot_token: &str, message_content: &str) -> Self {
        let db = db::DBConnection::new().unwrap();
        let channels = db.get_channels().unwrap();

        Self {
            total_requests: channels.into_iter().map(|v| send_message(client, bot_token, message_content, v)).collect(),
        }
    }
}

impl Future for MessageBroadcast {
    type Item = ();
    type Error = ();

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        let mut i = 0;
        while i != self.total_requests.len() {
            if i >= REQUEST_COUNT { return Ok(Async::NotReady); }
            match self.total_requests[i].poll() {
                Ok(Async::Ready(_)) => {
                    self.total_requests.remove(i);
                },
                Ok(Async::NotReady) => {
                    i += 1;
                },
                Err(_) => {
                    self.total_requests.remove(i);
                }
            };
        }
        if self.total_requests.len() <= 0 {
            return Ok(Async::Ready(()));
        }
        Ok(Async::NotReady)
    }
}

struct HyperRuntime {
    client: HyperClient,
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

            if let RequestState::ImageBroadcast(filename) = request {
                let message = "https://johnwickbot.shop/".to_owned() + &filename;
                tokio::spawn(MessageBroadcast::new(&self.client, &self.bot_token, &message));
            }
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
        client: http_client,
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
