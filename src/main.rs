extern crate serenity;
extern crate hyper;
extern crate hyper_tls;
extern crate futures;
extern crate tokio;
extern crate serde_json;

mod client;
mod db;

use std::sync::{Mutex, Arc};
use std::{thread, time};
use serenity::Client;
use serenity::prelude::{TypeMapKey, EventHandler, Context};
use serenity::model::channel::Message;
use serde_json::json;
use hyper::rt;
use hyper::http::Request;
use futures::{Async, Stream};
use futures::future::Future;
use futures::sync::mpsc;

type HyperClient = hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>;

struct JWHandler;

impl EventHandler for JWHandler {
    fn message(&self, ctx: Context, msg: Message) {
        if msg.content == "!ping" {
            let mut data = ctx.data.write();
            let hyper_state = data.get_mut::<HyperState>().unwrap();
            let mut req_state = hyper_state.lock().unwrap();
            req_state.requests.push(RequestState {
                channel_id: msg.channel_id.0,
                message_content: "Pong!".to_owned(),
            });
        }
    }
}

struct HyperState;

impl TypeMapKey for HyperState {
    type Value = Arc<Mutex<RequestQueue>>;
}

struct RequestState {
    channel_id: u64,
    message_content: String,
}

struct RequestQueue {
    requests: Vec<RequestState>,
}

impl RequestQueue {
    fn new() -> Self {
        Self {
            requests: Vec::new(),
        }
    }
}

struct HyperRuntime {
    requests: Arc<Mutex<RequestQueue>>,
    client: HyperClient,
    bot_token: String,
    receiver: mpsc::UnboundedReceiver,
}

impl HyperRuntime {
    fn send_message(&self, message: RequestState) -> impl Future<Item = (), Error = ()> {
        let uri = "https://discordapp.com/api/v6/channels/".to_owned() + &message.channel_id.to_string() + "/messages";
        let request_body = json!({
            "content": message.message_content
        }).to_string();
        let request = Request::builder()
            .uri(uri)
            .method("POST")
            .header("Authorization", "Bot ".to_owned() + &self.bot_token)
            .header("Content-Type", "application/json")
            .header("User-Agent", "JohnWickBot(https://johnwickbot.shop, 0.1)")
            .body(hyper::Body::from(request_body))
            .unwrap();

        self.client.request(request)
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
            })
    }
}

impl Future for HyperRuntime {
    type Item = ();

    type Error = ();

    fn poll(&mut self) -> Result<Async<Self::Item>, Self::Error> {
        loop {
            let to_run = {
                let mut requests_queue = self.requests.lock().unwrap();
                let mut data = Vec::new();
                data.append(&mut requests_queue.requests);

                data
            };

            println!("Requests to run: {}", to_run.len());

            for request in to_run {
                rt::spawn(self.send_message(request));
            }

            thread::sleep(time::Duration::from_millis(2000));
        }
        Ok(Async::NotReady)
    }
}

fn main() {
    let bot_token = std::env::var("DISCORD_TOKEN").unwrap();
    let request_state = Arc::new(Mutex::new(RequestQueue::new()));
    let mut discord_client = Client::new(bot_token.clone(), JWHandler).unwrap();
    
    {
        let mut data = discord_client.data.write();
        data.insert::<HyperState>(request_state.clone());
    }

    let https = hyper_tls::HttpsConnector::new(4).unwrap();
    let http_client = hyper::Client::builder().build::<_, hyper::Body>(https);

    let unb_channel = mspc::unbounded<RequestQueue>();

    let hyper_runtime = HyperRuntime {
        requests: request_state,
        client: http_client,
        bot_token,
        receiver: unb_channel.1,
    };

    let client_runtime = client::ClientFuture::new().map_err(|_| println!("Client Error"));

    thread::spawn(move || {
        rt::run(client_runtime.join(hyper_runtime).and_then(|_| {
            println!("Runtimes finished");
            Ok(())
        }));
    });

    discord_client.start().unwrap();
}
