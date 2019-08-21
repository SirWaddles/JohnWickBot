extern crate serenity;
extern crate hyper;
extern crate hyper_tls;
extern crate futures;
extern crate tokio;
extern crate serde_json;

mod client;
mod db;

use std::thread;
use serenity::Client;
use serenity::prelude::{EventHandler, Context};
use serenity::model::channel::Message;
use serde_json::json;
use hyper::rt;
use hyper::http::Request;
use futures::{Async, Stream};
use futures::future::Future;
use futures::sync::mpsc;
use client::RequestState;

type HyperClient = hyper::Client<hyper_tls::HttpsConnector<hyper::client::HttpConnector>>;

struct JWHandler;

impl EventHandler for JWHandler {
    fn message(&self, ctx: Context, msg: Message) {
        if msg.content == "!ping" {
            
        }
    }
}

struct HyperRuntime {
    client: HyperClient,
    bot_token: String,
    receiver: mpsc::UnboundedReceiver<RequestState>,
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
            let request = match self.receiver.poll() {
                Ok(Async::Ready(Some(val))) => val,
                Ok(Async::Ready(None)) => return Ok(Async::Ready(())),
                Ok(Async::NotReady) => break,
                Err(e) => return Err(e),
            };

            rt::spawn(self.send_message(request));
        }
        Ok(Async::NotReady)
    }
}

fn main() {
    let bot_token = std::env::var("DISCORD_TOKEN").unwrap();
    let mut discord_client = Client::new(bot_token.clone(), JWHandler).unwrap();
    let https = hyper_tls::HttpsConnector::new(4).unwrap();
    let http_client = hyper::Client::builder().build::<_, hyper::Body>(https);

    let unb_channel = mpsc::unbounded::<RequestState>();

    let hyper_runtime = HyperRuntime {
        client: http_client,
        bot_token,
        receiver: unb_channel.1,
    };

    let client_runtime = client::ClientFuture::new(unb_channel.0).map_err(|_| println!("Client Error"));

    thread::spawn(move || {
        rt::run(client_runtime.join(hyper_runtime).and_then(|_| {
            println!("Runtimes finished");
            Ok(())
        }));
    });

    discord_client.start().unwrap();
}
